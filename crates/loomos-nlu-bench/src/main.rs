//! nlu-bench —— 同一份 loomos-nlu 业务代码的三种运行方式基准：
//!   native  : 直接调用 RuleNlu::parse + serde_json 序列化（与 wasm 路径逐字节对齐）
//!   interp  : WAMR 解释器跑 module.wasm（业务代码经 loomos-nlu-wasm-wrap 导出）
//!   aot     : WAMR 跑 wamrc 产出的 .aot
//!
//! 默认构建只含 native 模式；`--features wamr`（需 IWASM_LIB_DIR 指向 libvmlib.a）
//! 才编译 interp/aot 路径——保证 workspace 门禁在无 WAMR 库的机器上也能跑全量。
//!
//! 采集纪律：不只测函数速度——Runtime 初始化、模块读取、模块加载、WASI 绑定、
//! 实例化、exec_env 创建、函数查找、guest 业务构造/缓冲分配、预热、稳态延迟
//! （P50/P95/P99）、吞吐窗口、CPU 时间、空闲/预热后/峰值 RSS 全部分段记录，
//! 末尾输出单行 JSON；外部进程级启动时间（exec+loader+libc）由包装脚本另测。

#![allow(static_mut_refs)]

use loomos_nlu::{EntityRegistry, RuleNlu};
use std::time::Instant;

// ---------------- 宿主指标（无第三方依赖：Linux 读 /proc，Windows 手写 extern） ----------------

#[cfg(unix)]
mod host {
    pub fn rss_kb() -> Option<u64> {
        status_field("VmRSS:")
    }
    pub fn peak_rss_kb() -> Option<u64> {
        status_field("VmHWM:")
    }

    fn status_field(key: &str) -> Option<u64> {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix(key) {
                let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                return num.parse().ok();
            }
        }
        None
    }

    /// 返回 (user_ms, system_ms)。USER_HZ 固定 100（Debian/Ubuntu 默认）。
    pub fn cpu_times_ms() -> Option<(u64, u64)> {
        let s = std::fs::read_to_string("/proc/self/stat").ok()?;
        let inner = s.rfind(')')?; // comm 可能含空格，从最后一个 ) 之后取字段
        let rest = s[inner + 1..].trim();
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 13 {
            return None;
        }
        let utime: u64 = fields[11].parse().ok()?; // 字段 14
        let stime: u64 = fields[12].parse().ok()?; // 字段 15
        const USER_HZ: u64 = 100;
        Some((utime * 1000 / USER_HZ, stime * 1000 / USER_HZ))
    }
}

#[cfg(windows)]
mod host {
    //! 手写 Win32 导入（不引 windows-sys——windows-gnu 工具链缺 dlltool 链，见 normos 纪律）。
    //! WorkingSetSize 是 Windows 语境下的 RSS 等价物；PeakWorkingSetSize 对应 VmHWM。

    const PROCESS_MEMORY_COUNTERS_SIZE: u32 = 72; // 64 位下 sizeof(PROCESS_MEMORY_COUNTERS)

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn GetProcessTimes(
            h: *mut core::ffi::c_void,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            h: *mut core::ffi::c_void,
            p: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    pub fn rss_kb() -> Option<u64> {
        let mut c = ProcessMemoryCounters {
            cb: PROCESS_MEMORY_COUNTERS_SIZE,
            page_fault_count: 0,
            peak_working_set: 0,
            working_set: 0,
            quota_peak_paged_pool: 0,
            quota_paged_pool: 0,
            quota_peak_non_paged_pool: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        unsafe {
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, PROCESS_MEMORY_COUNTERS_SIZE) != 0
            {
                Some((c.working_set / 1024) as u64)
            } else {
                None
            }
        }
    }

    pub fn peak_rss_kb() -> Option<u64> {
        let mut c = ProcessMemoryCounters {
            cb: PROCESS_MEMORY_COUNTERS_SIZE,
            page_fault_count: 0,
            peak_working_set: 0,
            working_set: 0,
            quota_peak_paged_pool: 0,
            quota_paged_pool: 0,
            quota_peak_non_paged_pool: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        unsafe {
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, PROCESS_MEMORY_COUNTERS_SIZE) != 0
            {
                Some((c.peak_working_set / 1024) as u64)
            } else {
                None
            }
        }
    }

    /// 返回 (user_ms, system_ms)。FILETIME 单位 100ns。
    pub fn cpu_times_ms() -> Option<(u64, u64)> {
        let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
        unsafe {
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
        }
        Some((user / 10_000, kernel / 10_000))
    }
}

// ---------------- WAMR C API 绑定（随 vmlib 静态库链接；wamr feature 门控） ----------------

#[cfg(feature = "wamr")]
type WasmModule = *mut u8;
#[cfg_attr(not(feature = "wamr"), allow(dead_code))]
type WasmModuleInst = *mut u8;
#[cfg_attr(not(feature = "wamr"), allow(dead_code))]
type WasmFunction = *mut u8;
#[cfg_attr(not(feature = "wamr"), allow(dead_code))]
type WasmExecEnv = *mut u8;

/// 与 core/iwasm/include/wasm_export.h 的 RuntimeInitArgs 逐字段对齐（64 位布局）。
/// mem_alloc_option 取 union 最大成员（3 指针 = 24B，WASM_MEM_ALLOC_WITH_USER_DATA 关闭）。
#[cfg(feature = "wamr")]
#[repr(C)]
struct RuntimeInitArgs {
    mem_alloc_type: u32,
    mem_alloc_option: [u8; 24],
    native_module_name: *const u8,
    native_symbols: *const u8,
    n_native_symbols: u32,
    max_thread_num: u32,
    ip_addr: [u8; 128],
    unused: i32,
    instance_port: i32,
    fast_jit_code_cache_size: u32,
    gc_heap_size: u32,
    running_mode: u32,
    llvm_jit_opt_level: u32,
    llvm_jit_size_level: u32,
    segue_flags: u32,
    enable_linux_perf: bool,
}

#[cfg(feature = "wamr")]
const ALLOC_WITH_SYSTEM_ALLOCATOR: u32 = 2;

#[cfg(feature = "wamr")]
#[link(name = "iwasm")]
unsafe extern "C" {
    fn wasm_runtime_full_init(init_args: *mut RuntimeInitArgs) -> bool;
    fn wasm_runtime_load(
        buf: *mut u8,
        size: u32,
        error_buf: *mut u8,
        error_buf_size: u32,
    ) -> WasmModule;
    fn wasm_runtime_unload(module: WasmModule);
    fn wasm_runtime_set_wasi_args(
        module: WasmModule,
        dir_list: *const *const u8,
        dir_count: u32,
        map_dir_list: *const *const u8,
        map_dir_count: u32,
        env: *const *const u8,
        env_count: u32,
        argv: *const *const u8,
        argc: i32,
    );
    fn wasm_runtime_instantiate(
        module: WasmModule,
        stack_size: u32,
        heap_size: u32,
        error_buf: *mut u8,
        error_buf_size: u32,
    ) -> WasmModuleInst;
    fn wasm_runtime_deinstantiate(inst: WasmModuleInst);
    fn wasm_runtime_lookup_function(inst: WasmModuleInst, name: *const u8) -> WasmFunction;
    fn wasm_runtime_create_exec_env(inst: WasmModuleInst, stack_size: u32) -> WasmExecEnv;
    fn wasm_runtime_destroy_exec_env(exec_env: WasmExecEnv);
    fn wasm_runtime_call_wasm(
        exec_env: WasmExecEnv,
        function: WasmFunction,
        argc: u32,
        argv: *mut u32,
    ) -> bool;
    fn wasm_runtime_addr_app_to_native(inst: WasmModuleInst, offset: u64) -> *mut u8;
    fn wasm_runtime_get_exception(inst: WasmModuleInst) -> *const u8;
}

// ---------------- 业务固定语料 ----------------

const FIXTURES_JSON: &str = include_str!("../fixtures.json");

fn fixtures() -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(FIXTURES_JSON).expect("fixtures.json");
    v["utterances"]
        .as_array()
        .expect("utterances array")
        .iter()
        .map(|s| s.as_str().expect("string").to_string())
        .collect()
}

// ---------------- 分位数（nearest-rank） ----------------

fn percentile(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p * sorted.len() as u64).div_ceil(100)).max(1) as usize - 1;
    sorted[idx.min(sorted.len() - 1)]
}

// ---------------- 主流程 ----------------

const ITERS_DEFAULT: usize = 2000; // 延迟样本数
const TP_SECS_DEFAULT: f64 = 4.0; // 吞吐窗口（秒）
const WARM_ROUNDS: usize = 3; // 预热 = 全语料 × 3

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "用法: nlu-bench native | interp <module.wasm> | aot <module.aot> [iters] [tp_secs]"
        );
        std::process::exit(1);
    }
    let mode = args[1].clone();
    let module_path = if mode == "interp" || mode == "aot" {
        if args.len() < 3 {
            eprintln!("interp/aot 模式需要模块路径参数");
            std::process::exit(1);
        }
        Some(args[2].clone())
    } else {
        None
    };
    let iters: usize = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(ITERS_DEFAULT);
    let tp_secs: f64 = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(TP_SECS_DEFAULT);

    let t_entry = Instant::now();
    let fixtures = fixtures();
    let max_input_len = fixtures.iter().map(|s| s.len()).max().unwrap_or(256) + 8;

    let mut phases: Vec<(String, u64)> = Vec::new();
    let mut module_bytes: u64 = 0;
    let mut ctx = ModeCtx::setup(
        &mode,
        module_path.as_deref(),
        max_input_len,
        &mut phases,
        &mut module_bytes,
    );
    let ready_us = t_entry.elapsed().as_micros() as u64;

    let rss_idle = host::rss_kb();

    // ---- 预热：全语料 × WARM_ROUNDS，不计入样本 ----
    let warm_start = Instant::now();
    for _ in 0..WARM_ROUNDS {
        for f in &fixtures {
            ctx.parse_once(f.as_bytes());
        }
    }
    phases.push(("warmup".into(), warm_start.elapsed().as_micros() as u64));
    let rss_after_warmup = host::rss_kb();

    // ---- 稳态延迟：iters 次，语料轮转（首调/冷启动影响已被预热隔离） ----
    let mut durations = Vec::with_capacity(iters);
    for i in 0..iters {
        let f = &fixtures[i % fixtures.len()];
        let t = Instant::now();
        ctx.parse_once(f.as_bytes());
        durations.push(t.elapsed().as_nanos() as u64);
    }
    durations.sort_unstable();
    let lat = serde_json::json!({
        "n": durations.len(),
        "p50_ns": percentile(&durations, 50),
        "p95_ns": percentile(&durations, 95),
        "p99_ns": percentile(&durations, 99),
        "min_ns": durations.first().copied().unwrap_or(0),
        "max_ns": durations.last().copied().unwrap_or(0),
        "mean_ns": durations.iter().sum::<u64>() / durations.len().max(1) as u64,
    });

    // ---- 吞吐窗口 + CPU 占用 ----
    let cpu_tp_start = host::cpu_times_ms();
    let tp_start = Instant::now();
    let mut calls: u64 = 0;
    let mut checksum: u64 = 0;
    while tp_start.elapsed().as_secs_f64() < tp_secs {
        for f in &fixtures {
            checksum = checksum
                .wrapping_mul(31)
                .wrapping_add(ctx.parse_once(f.as_bytes()) as u64);
            calls += 1;
        }
    }
    let tp_wall = tp_start.elapsed().as_secs_f64();
    let cpu_tp_end = host::cpu_times_ms();
    let cpu_ms_win = match (cpu_tp_start, cpu_tp_end) {
        (Some((u0, s0)), Some((u1, s1))) => u1.saturating_sub(u0) + s1.saturating_sub(s0),
        _ => 0,
    };
    let cpu_pct = if tp_wall > 0.0 {
        (cpu_ms_win as f64 / 1000.0) / tp_wall * 100.0
    } else {
        0.0
    };

    // ---- 汇总（含运行时销毁成本） ----
    let teardown_start = Instant::now();
    ctx.teardown();
    phases.push((
        "teardown".into(),
        teardown_start.elapsed().as_micros() as u64,
    ));

    let rss_peak = host::peak_rss_kb();
    let cpu_total = host::cpu_times_ms();
    let total_cpu_ms = match cpu_total {
        Some((u, s)) => u + s,
        None => 0,
    };
    let proc_wall_us = t_entry.elapsed().as_micros() as u64;

    let phases_map: serde_json::Map<String, serde_json::Value> = phases
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::json!(v)))
        .collect();
    let out = serde_json::json!({
        "meta": {
            "mode": mode,
            "module": module_path,
            "module_bytes": module_bytes,
            "os": if cfg!(windows) { "windows" } else { "linux" },
            "arch": if cfg!(target_arch = "x86_64") { "x86_64" }
                    else if cfg!(target_arch = "aarch64") { "aarch64" }
                    else { "other" },
            "iters": iters,
            "tp_secs": tp_secs,
            "fixtures": fixtures.len(),
            "checksum": checksum,
            "proc_wall_us": proc_wall_us,
            "ready_us": ready_us,
        },
        "phases_us": phases_map,
        "rss_kb": {
            "idle": rss_idle,
            "after_warmup": rss_after_warmup,
            "peak": rss_peak,
        },
        "cpu_ms_total_user_sys": total_cpu_ms,
        "latency": lat,
        "throughput": {
            "calls": calls,
            "wall_s": tp_wall,
            "calls_per_sec": if tp_wall > 0.0 { calls as f64 / tp_wall } else { 0.0 },
            "cpu_ms_window": cpu_ms_win,
            "cpu_percent": cpu_pct,
            "cpu_ns_per_call": cpu_ms_win.checked_mul(1_000_000).map(|v| v / calls).unwrap_or(0),
        },
    });
    println!("{}", out);
}

// ---------------- 模式上下文 ----------------

struct OutBuf {
    buf: Vec<u8>,
}

impl OutBuf {
    fn new() -> Self {
        Self {
            buf: vec![0u8; 8192],
        }
    }
}

enum ModeCtx {
    Native {
        nlu: RuleNlu,
        out: OutBuf,
    },
    #[cfg(feature = "wamr")]
    Wamr {
        buf: Vec<u8>, // wasm_runtime_load 之后仍需持有（runtime 可能引用其内容）
        module: WasmModule,
        inst: WasmModuleInst,
        exec_env: WasmExecEnv,
        in_ptr: u32,
        in_native: *mut u8,
        out_native: *mut u8,
        parse_fn: WasmFunction,
        out_host: OutBuf,
    },
}

impl ModeCtx {
    #[cfg_attr(not(feature = "wamr"), allow(unused_variables))]
    fn setup(
        mode: &str,
        module_path: Option<&str>,
        max_input_len: usize,
        phases: &mut Vec<(String, u64)>,
        module_bytes: &mut u64,
    ) -> Self {
        #[allow(unused_mut, unused_variables)]
        match mode {
            "native" => {
                let t = Instant::now();
                let nlu = RuleNlu::new(EntityRegistry::default_home());
                phases.push(("nlu_construct".into(), t.elapsed().as_micros() as u64));
                Self::Native {
                    nlu,
                    out: OutBuf::new(),
                }
            }
            #[cfg(not(feature = "wamr"))]
            "interp" | "aot" => {
                eprintln!(
                    "本构建未启用 wamr feature：interp/aot 不可用（--features wamr 且设置 IWASM_LIB_DIR）"
                );
                std::process::exit(1);
            }
            #[cfg(feature = "wamr")]
            "interp" | "aot" => {
                let path = module_path.expect("module path");
                let t = Instant::now();
                let mut buf = std::fs::read(path).unwrap_or_else(|e| {
                    eprintln!("读模块失败 {path}: {e}");
                    std::process::exit(2);
                });
                *module_bytes = buf.len() as u64;
                phases.push(("module_read".into(), t.elapsed().as_micros() as u64));

                // runtime 初始化
                let t = Instant::now();
                let mut init_args = unsafe { std::mem::zeroed::<RuntimeInitArgs>() };
                init_args.mem_alloc_type = ALLOC_WITH_SYSTEM_ALLOCATOR;
                let ok = unsafe { wasm_runtime_full_init(&mut init_args) };
                if !ok {
                    eprintln!("wasm_runtime_full_init 失败");
                    std::process::exit(2);
                }
                phases.push(("runtime_init".into(), t.elapsed().as_micros() as u64));

                // 模块加载（interp=wasm 字节码 / aot=AOT 镜像）
                let t = Instant::now();
                let mut err = [0u8; 128];
                let module = unsafe {
                    wasm_runtime_load(buf.as_mut_ptr(), buf.len() as u32, err.as_mut_ptr(), 128)
                };
                if module.is_null() {
                    eprintln!("wasm_runtime_load 失败: {}", cstr(&err));
                    std::process::exit(2);
                }
                phases.push(("module_load".into(), t.elapsed().as_micros() as u64));

                // WASI 绑定（load 后、instantiate 前）
                let t = Instant::now();
                let prog = b"nlu-bench\0";
                let argv = [prog.as_ptr()];
                unsafe {
                    wasm_runtime_set_wasi_args(
                        module,
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        0,
                        argv.as_ptr(),
                        1,
                    );
                }
                phases.push(("wasi_args".into(), t.elapsed().as_micros() as u64));

                // 实例化：栈 64KB，heap 0（Rust 模块自管线性内存，不依赖 WAMR app heap）
                let t = Instant::now();
                let mut err2 = [0u8; 128];
                let inst = unsafe {
                    wasm_runtime_instantiate(module, 64 * 1024, 0, err2.as_mut_ptr(), 128)
                };
                if inst.is_null() {
                    eprintln!("wasm_runtime_instantiate 失败: {}", cstr(&err2));
                    std::process::exit(2);
                }
                phases.push(("instantiate".into(), t.elapsed().as_micros() as u64));

                // exec_env
                let t = Instant::now();
                let exec_env = unsafe { wasm_runtime_create_exec_env(inst, 64 * 1024) };
                if exec_env.is_null() {
                    eprintln!("wasm_runtime_create_exec_env 失败");
                    std::process::exit(2);
                }
                phases.push(("exec_env_create".into(), t.elapsed().as_micros() as u64));

                // 导出函数查找
                let t = Instant::now();
                let init_fn =
                    unsafe { wasm_runtime_lookup_function(inst, b"ab_init_registry\0".as_ptr()) };
                let alloc_fn =
                    unsafe { wasm_runtime_lookup_function(inst, b"ab_alloc_input\0".as_ptr()) };
                let parse_fn =
                    unsafe { wasm_runtime_lookup_function(inst, b"ab_parse\0".as_ptr()) };
                let outp_fn =
                    unsafe { wasm_runtime_lookup_function(inst, b"ab_out_ptr\0".as_ptr()) };
                if parse_fn.is_null()
                    || alloc_fn.is_null()
                    || outp_fn.is_null()
                    || init_fn.is_null()
                {
                    eprintln!("导出函数查找失败（模块里没有 ab_* 导出？）");
                    std::process::exit(2);
                }
                phases.push(("func_lookup".into(), t.elapsed().as_micros() as u64));

                // guest 业务构造 + 缓冲分配（NLU 构造发生在 guest 内——wasm 路径的业务成本）
                let t = Instant::now();
                let mut argv_buf: [u32; 8] = [0; 8];
                assert!(unsafe {
                    wasm_runtime_call_wasm(exec_env, init_fn, 0, argv_buf.as_mut_ptr())
                });

                argv_buf[0] = max_input_len as u32;
                assert!(unsafe {
                    wasm_runtime_call_wasm(exec_env, alloc_fn, 1, argv_buf.as_mut_ptr())
                });
                let in_ptr = argv_buf[1];
                let in_native = unsafe { wasm_runtime_addr_app_to_native(inst, in_ptr as u64) };

                assert!(unsafe {
                    wasm_runtime_call_wasm(exec_env, outp_fn, 0, argv_buf.as_mut_ptr())
                });
                let out_ptr = argv_buf[1];
                let out_native = unsafe { wasm_runtime_addr_app_to_native(inst, out_ptr as u64) };
                phases.push(("guest_setup".into(), t.elapsed().as_micros() as u64));

                Self::Wamr {
                    buf,
                    module,
                    inst,
                    exec_env,
                    in_ptr,
                    in_native,
                    out_native,
                    parse_fn,
                    out_host: OutBuf::new(),
                }
            }
            _ => {
                eprintln!("未知模式 {mode}");
                std::process::exit(1);
            }
        }
    }

    /// 单次解析，返回结果字节数（作校验和输入）。
    fn parse_once(&mut self, text: &[u8]) -> usize {
        match self {
            Self::Native { nlu, out } => {
                let decision: loomos_nlu::NluDecision =
                    nlu.parse(std::str::from_utf8(text).expect("utf8 fixture"));
                let json = serde_json::to_vec(&decision).expect("serialize");
                let n = json.len().min(out.buf.len());
                out.buf[..n].copy_from_slice(&json[..n]);
                n
            }
            #[cfg(feature = "wamr")]
            Self::Wamr {
                exec_env,
                inst,
                parse_fn,
                in_ptr,
                in_native,
                out_native,
                out_host,
                ..
            } => {
                unsafe {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), *in_native, text.len());
                }
                let mut argv: [u32; 4] = [*in_ptr, text.len() as u32, 0, 0];
                let ok =
                    unsafe { wasm_runtime_call_wasm(*exec_env, *parse_fn, 2, argv.as_mut_ptr()) };
                if !ok {
                    let msg = unsafe { wasm_runtime_get_exception(*inst) };
                    if !msg.is_null() {
                        eprintln!("wasm 异常: {}", unsafe { cstr_ptr(msg) });
                    } else {
                        eprintln!("wasm_runtime_call_wasm 失败");
                    }
                    std::process::exit(3);
                }
                let out_len = argv[2] as usize;
                // 读回结果（模拟 executor 消费 JSON 字节的真实数据通路）
                if out_len <= out_host.buf.len() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            *out_native,
                            out_host.buf.as_mut_ptr(),
                            out_len,
                        );
                    }
                }
                out_len
            }
        }
    }

    fn teardown(&mut self) {
        #[cfg(feature = "wamr")]
        if let Self::Wamr {
            module,
            inst,
            exec_env,
            ..
        } = self
        {
            unsafe {
                wasm_runtime_destroy_exec_env(*exec_env);
                wasm_runtime_deinstantiate(*inst);
                wasm_runtime_unload(*module);
            }
        }
    }
}

#[cfg_attr(not(feature = "wamr"), allow(dead_code))]
fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(feature = "wamr")]
unsafe fn cstr_ptr(p: *const u8) -> String {
    unsafe {
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        cstr(std::slice::from_raw_parts(p, len))
    }
}
