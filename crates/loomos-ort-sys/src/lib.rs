//! loomos-ort-sys：onnxruntime.dll 的最小 unsafe 绑定（LoadLibrary 动态加载）。
//!
//! **全仓第二个声明式可信边界 crate**（与 loomos-sys 同地位；其余 crate
//! `#![forbid(unsafe_code)]`）。只暴露 vision 引擎需要的最小面：
//! env/session/options/内存信息/张量创建/Run/形状查询/错误消息。
//!
//! Windows：`LoadLibraryA + GetProcAddress`（kernel32 默认导入，无 dlltool 链问题——
//! 见 EVIDENCE.md 工程决策；getrandom/windows-sys 系被否决的原因与此相反）。
//! Linux（板端）：cfg(unix) 走 dlopen（待板端阶段启用）。
//!
//! OrtApi 表：v20 全部为函数指针，槽位索引由 scripts/gen_ort_sys.py 从
//! onnxruntime_c_api.h 解析生成（gen.rs，勿手改）。
//!
//! 证据纪律：每次调用失败都带 GetErrorMessage 文本；不做任何"成功假设"。

#![deny(unsafe_op_in_unsafe_fn)]
// -sys crate 约定：公开包装函数内部解引用裸指针（有效性由实现内部保证并文档化），
// clippy 的保守 lint 在此显式豁免——安全边界就是本 crate。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[cfg(target_os = "windows")]
pub mod windows_impl {
    use std::ffi::c_void;
    use std::path::Path;
    use thiserror::Error;

    include!("gen.rs");

    #[derive(Error, Debug, Clone)]
    pub enum OrtSysError {
        #[error("onnxruntime.dll 加载失败: {0}")]
        Load(String),
        #[error("OrtGetApiBase/GetApi 失败 (v{version})")]
        GetApi { version: u32 },
        #[error("ORT 调用失败: {msg}")]
        Status { msg: String },
        #[error("ORT I/O 不符: {0}")]
        Shape(String),
    }

    // kernel32 默认导入（mingw 自带 import lib；nlu-bench 已有同款先例）
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    type FnGetApiBase = unsafe extern "system" fn() -> *const c_void;
    type FnGetApi = unsafe extern "system" fn(version: u32) -> *const c_void;
    type FnGetErrorMessage = unsafe extern "system" fn(*const c_void) -> *const u8;
    type FnReleaseStatus = unsafe extern "system" fn(*mut c_void);
    type FnCreateEnv = unsafe extern "system" fn(u32, *const u8, *mut *mut c_void) -> *mut c_void;
    type FnReleaseEnv = unsafe extern "system" fn(*mut c_void);
    type FnCreateSessionOptions = unsafe extern "system" fn(*mut *mut c_void) -> *mut c_void;
    type FnReleaseSessionOptions = unsafe extern "system" fn(*mut c_void);
    type FnSetIntraOpNumThreads = unsafe extern "system" fn(*mut c_void, i32) -> *mut c_void;
    type FnSetSessionGraphOptimizationLevel =
        unsafe extern "system" fn(*mut c_void, u32) -> *mut c_void;
    type FnCreateSession = unsafe extern "system" fn(
        *const c_void,
        *const u16,
        *mut c_void,
        *mut *mut c_void,
    ) -> *mut c_void;
    type FnReleaseSession = unsafe extern "system" fn(*mut c_void);
    type FnSessionGetInputCount = unsafe extern "system" fn(*const c_void) -> usize;
    type FnSessionGetInputName =
        unsafe extern "system" fn(*const c_void, usize, *mut c_void, *mut *mut u8) -> *mut c_void;
    type FnSessionGetOutputCount = unsafe extern "system" fn(*const c_void) -> usize;
    type FnSessionGetOutputName =
        unsafe extern "system" fn(*const c_void, usize, *mut c_void, *mut *mut u8) -> *mut c_void;
    type FnGetAllocatorWithDefaultOptions = unsafe extern "system" fn() -> *mut c_void;
    type FnSessionGetInputTypeInfo =
        unsafe extern "system" fn(*const c_void, usize, *mut *mut c_void) -> *mut c_void;
    type FnCastTypeInfoToTensorInfo = unsafe extern "system" fn(*const c_void) -> *mut c_void;
    type FnGetDimensionsCount = unsafe extern "system" fn(*const c_void) -> usize;
    type FnGetDimensions = unsafe extern "system" fn(*const c_void, *mut i64, usize);
    type FnGetTensorElementType = unsafe extern "system" fn(*const c_void) -> i32;
    type FnCreateCpuMemoryInfo =
        unsafe extern "system" fn(u32, u32, *mut *mut c_void) -> *mut c_void;
    type FnReleaseMemoryInfo = unsafe extern "system" fn(*mut c_void);
    type FnCreateTensorWithDataAsOrtValue = unsafe extern "system" fn(
        *const c_void,
        *mut c_void,
        usize,
        *const i64,
        usize,
        i32,
        *mut *mut c_void,
    ) -> *mut c_void;
    type FnReleaseValue = unsafe extern "system" fn(*mut c_void);
    type FnRun = unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const *const u8,
        *const *const c_void,
        usize,
        *const *const u8,
        *mut *mut c_void,
        usize,
    ) -> *mut c_void;
    type FnGetTensorMutableData =
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> *mut c_void;
    type FnGetTensorTypeAndShape = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
    type FnGetTensorShapeElementCount = unsafe extern "system" fn(*mut c_void) -> usize;

    /// 已加载的 OrtApi 表句柄。
    pub struct Ort {
        api: *const OrtApi,
        /// dll 句柄（生命周期内保持加载；进程退出统一卸载）
        #[allow(dead_code)]
        module: *mut c_void,
    }

    // 跨线程共享：onnxruntime 表本身只读
    unsafe impl Send for Ort {}
    unsafe impl Sync for Ort {}

    /// 裸句柄包装：Send+Sync 由 ORT CPU EP 语义支撑（vision 侧组合用，
    /// 使其在不写 unsafe impl 的情况下自动获得 Send/Sync——forbid 纪律保持）。
    pub struct RawHandle(pub *mut c_void);
    unsafe impl Send for RawHandle {}
    unsafe impl Sync for RawHandle {}

    /// OrtApiBase 槽位视图（GetApi 首槽位）。
    #[repr(C)]
    pub struct OrtApiBase {
        pub get_api: unsafe extern "system" fn(u32) -> *const c_void,
        pub get_version_string: unsafe extern "system" fn() -> *const u8,
    }

    impl Ort {
        /// LoadLibrary + OrtGetApiBase()->GetApi(ORT_API_VERSION)。
        pub fn load(dll_path: &Path) -> Result<Self, OrtSysError> {
            let path = dll_path
                .to_str()
                .ok_or_else(|| OrtSysError::Load("路径非 UTF-8".into()))?;
            let mut pathz = path.as_bytes().to_vec();
            pathz.push(0);
            // SAFETY: pathz 为 NUL 结尾的合法 C 字符串
            let module = unsafe { LoadLibraryA(pathz.as_ptr()) };
            if module.is_null() {
                return Err(OrtSysError::Load(format!(
                    "LoadLibraryA({path}) 失败（onnxruntime.dll 是否存在？）"
                )));
            }
            // SAFETY: module 已加载，导出名常量
            let get_api_base =
                unsafe { GetProcAddress(module, c"OrtGetApiBase".as_ptr() as *const u8) };
            if get_api_base.is_null() {
                return Err(OrtSysError::Load("无 OrtGetApiBase 导出".into()));
            }
            // SAFETY: 符号地址 → 函数指针，签名与导出一致
            let get_api_base: FnGetApiBase = unsafe { std::mem::transmute(get_api_base) };
            // SAFETY: 调用合法导出
            let base = unsafe { get_api_base() };
            if base.is_null() {
                return Err(OrtSysError::GetApi {
                    version: ORT_API_VERSION,
                });
            }
            // SAFETY: base 指向 OrtApiBase（GetApi 首槽位），签名一致
            let get_api: FnGetApi = unsafe { std::mem::transmute(base) };
            // SAFETY: 请求头文件同版本 API
            let api = unsafe { get_api(ORT_API_VERSION) };
            if api.is_null() {
                return Err(OrtSysError::GetApi {
                    version: ORT_API_VERSION,
                });
            }
            Ok(Self {
                api: api as *const OrtApi,
                module,
            })
        }

        fn slot_fn<T>(&self, idx: usize) -> T
        where
            T: Copy,
        {
            // SAFETY: api 指向已加载的只读函数指针表；调用签名由调用方按头文件保证
            unsafe { std::mem::transmute_copy::<usize, T>(&self.slot(idx)) }
        }

        fn slot(&self, idx: usize) -> usize {
            // SAFETY: api 指向加载后的只读函数指针表
            unsafe { (*self.api).slots[idx] }
        }

        fn check_status(&self, status: *mut c_void) -> Result<(), OrtSysError> {
            if status.is_null() {
                return Ok(());
            }
            // SAFETY: status 非空；GetErrorMessage 槽位，签名一致
            let msg = unsafe {
                self.slot_fn::<FnGetErrorMessage>(idx::GETERRORMESSAGE)(status as *const c_void)
            };
            let text = if msg.is_null() {
                String::new()
            } else {
                // SAFETY: ORT 返回的 NUL 结尾 UTF-8
                unsafe {
                    std::ffi::CStr::from_ptr(msg as *const i8)
                        .to_string_lossy()
                        .into_owned()
                }
            };
            // SAFETY: ReleaseStatus 槽位
            unsafe { self.slot_fn::<FnReleaseStatus>(idx::RELEASESTATUS)(status) };
            Err(OrtSysError::Status { msg: text })
        }

        pub fn create_env(&self) -> Result<*mut c_void, OrtSysError> {
            let mut env: *mut c_void = std::ptr::null_mut();
            let status = unsafe {
                self.slot_fn::<FnCreateEnv>(idx::CREATEENV)(
                    3,
                    c"loomos".as_ptr() as *const u8,
                    &mut env,
                )
            };
            self.check_status(status)?;
            Ok(env)
        }

        pub fn create_session(
            &self,
            env: *mut c_void,
            model_path: &Path,
            intra_threads: i32,
        ) -> Result<*mut c_void, OrtSysError> {
            // 约定：Status 返回 + out 参数
            let mut opts: *mut c_void = std::ptr::null_mut();
            let st = unsafe {
                self.slot_fn::<FnCreateSessionOptions>(idx::CREATESESSIONOPTIONS)(&mut opts)
            };
            self.check_status(st)?;
            if opts.is_null() {
                return Err(OrtSysError::Shape("CreateSessionOptions 返回空".into()));
            }
            let status = unsafe {
                self.slot_fn::<FnSetIntraOpNumThreads>(idx::SETINTRAOPNUMTHREADS)(
                    opts,
                    intra_threads,
                )
            };
            self.check_status(status)?;
            let opt_level = 99u32; // ORT_ENABLE_ALL
            let status = unsafe {
                self.slot_fn::<FnSetSessionGraphOptimizationLevel>(
                    idx::SETSESSIONGRAPHOPTIMIZATIONLEVEL,
                )(opts, opt_level)
            };
            self.check_status(status)?;

            let path16: Vec<u16> = model_path
                .to_str()
                .ok_or_else(|| OrtSysError::Load("模型路径非 UTF-8".into()))?
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut session: *mut c_void = std::ptr::null_mut();
            let status = unsafe {
                self.slot_fn::<FnCreateSession>(idx::CREATESESSION)(
                    env,
                    path16.as_ptr(),
                    opts,
                    &mut session,
                )
            };
            self.check_status(status)?;
            unsafe { self.slot_fn::<FnReleaseSessionOptions>(idx::RELEASESESSIONOPTIONS)(opts) };
            Ok(session)
        }

        pub fn io_counts(&self, session: *const c_void) -> (usize, usize) {
            // SAFETY: 计数查询，签名一致
            unsafe {
                (
                    self.slot_fn::<FnSessionGetInputCount>(idx::SESSIONGETINPUTCOUNT)(session),
                    self.slot_fn::<FnSessionGetOutputCount>(idx::SESSIONGETOUTPUTCOUNT)(session),
                )
            }
        }

        pub fn io_names(
            &self,
            session: *const c_void,
        ) -> Result<(Vec<String>, Vec<String>), OrtSysError> {
            let allocator = unsafe {
                self.slot_fn::<FnGetAllocatorWithDefaultOptions>(
                    idx::GETALLOCATORWITHDEFAULTOPTIONS,
                )()
            };
            let mut input_names = Vec::new();
            let mut output_names = Vec::new();
            let (nin, nout) = self.io_counts(session);
            for i in 0..nin {
                let mut name: *mut u8 = std::ptr::null_mut();
                let st = unsafe {
                    self.slot_fn::<FnSessionGetInputName>(idx::SESSIONGETINPUTNAME)(
                        session, i, allocator, &mut name,
                    )
                };
                self.check_status(st)?;
                input_names.push(self.take_and_free(allocator, name));
            }
            for i in 0..nout {
                let mut name: *mut u8 = std::ptr::null_mut();
                let st = unsafe {
                    self.slot_fn::<FnSessionGetOutputName>(idx::SESSIONGETOUTPUTNAME)(
                        session, i, allocator, &mut name,
                    )
                };
                self.check_status(st)?;
                output_names.push(self.take_and_free(allocator, name));
            }
            Ok((input_names, output_names))
        }

        /// 取 ORT 分配的字符串并用 OrtAllocator::Free 归还
        /// （槽位布局 version/Alloc/Free/... 由 gen.rs 按头文件序生成）。
        fn take_and_free(&self, allocator: *mut c_void, name: *mut u8) -> String {
            let s = if name.is_null() {
                String::new()
            } else {
                // SAFETY: ORT 分配的 NUL 结尾 UTF-8
                unsafe {
                    std::ffi::CStr::from_ptr(name as *const i8)
                        .to_string_lossy()
                        .into_owned()
                }
            };
            if !name.is_null() {
                // SAFETY: OrtAllocator::Free(self_, p)，槽位序来自头文件
                unsafe {
                    let alloc: OrtAllocator = *(allocator as *const OrtAllocator);
                    let free_fn: unsafe extern "system" fn(*mut c_void, *mut c_void) =
                        std::mem::transmute_copy::<usize, _>(&alloc.Free);
                    free_fn(allocator, name as *mut c_void);
                }
            }
            s
        }

        /// 输入张量形状（SessionGetInputTypeInfo → CastTypeInfoToTensorInfo → dims）。
        pub fn input_shape(
            &self,
            session: *const c_void,
            index: usize,
        ) -> Result<(Vec<i64>, i32), OrtSysError> {
            let mut type_info: *mut c_void = std::ptr::null_mut();
            let st = unsafe {
                self.slot_fn::<FnSessionGetInputTypeInfo>(idx::SESSIONGETINPUTTYPEINFO)(
                    session,
                    index,
                    &mut type_info,
                )
            };
            self.check_status(st)?;
            let tensor_info = unsafe {
                self.slot_fn::<FnCastTypeInfoToTensorInfo>(idx::CASTTYPEINFOTOTENSORINFO)(type_info)
            };
            let count = unsafe {
                self.slot_fn::<FnGetDimensionsCount>(idx::GETDIMENSIONSCOUNT)(tensor_info)
            };
            let mut dims = vec![0i64; count];
            // SAFETY: dims 与 count 匹配
            unsafe {
                self.slot_fn::<FnGetDimensions>(idx::GETDIMENSIONS)(
                    tensor_info,
                    dims.as_mut_ptr(),
                    count,
                )
            };
            let etype = unsafe {
                self.slot_fn::<FnGetTensorElementType>(idx::GETTENSORELEMENTTYPE)(tensor_info)
            };
            // type_info 由 session 持有，无需释放
            Ok((dims, etype))
        }

        pub fn create_cpu_memory_info(&self) -> Result<*mut c_void, OrtSysError> {
            // OrtArenaAllocator(1) + OrtMemTypeDefault(0)；Status 返回 + out 参数
            let mut info: *mut c_void = std::ptr::null_mut();
            let st = unsafe {
                self.slot_fn::<FnCreateCpuMemoryInfo>(idx::CREATECPUMEMORYINFO)(1, 0, &mut info)
            };
            self.check_status(st)?;
            if info.is_null() {
                return Err(OrtSysError::Shape("CreateCpuMemoryInfo 返回空".into()));
            }
            Ok(info)
        }

        /// 创建包装外部数据的输入张量（float32）。
        pub fn create_tensor_f32(
            &self,
            info: *mut c_void,
            data: &mut [f32],
            shape: &[i64],
        ) -> Result<*mut c_void, OrtSysError> {
            let mut value: *mut c_void = std::ptr::null_mut();
            let st = unsafe {
                self.slot_fn::<FnCreateTensorWithDataAsOrtValue>(
                    idx::CREATETENSORWITHDATAASORTVALUE,
                )(
                    info,
                    data.as_mut_ptr() as *mut c_void,
                    data.len(),
                    shape.as_ptr(),
                    shape.len(),
                    1, // ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT
                    &mut value,
                )
            };
            self.check_status(st)?;
            Ok(value)
        }

        pub fn release_value(&self, value: *mut c_void) {
            unsafe { self.slot_fn::<FnReleaseValue>(idx::RELEASEVALUE)(value) };
        }

        pub fn release_memory_info(&self, info: *mut c_void) {
            unsafe { self.slot_fn::<FnReleaseMemoryInfo>(idx::RELEASEMEMORYINFO)(info) };
        }

        /// Run（run_options 传 null）。
        pub fn run(
            &self,
            session: *mut c_void,
            input_names: &[*const u8],
            input_values: &[*mut c_void],
            output_names: &[*const u8],
            output_count: usize,
        ) -> Result<Vec<*mut c_void>, OrtSysError> {
            let mut outputs: Vec<*mut c_void> =
                (0..output_count).map(|_| std::ptr::null_mut()).collect();
            let status = unsafe {
                self.slot_fn::<FnRun>(idx::RUN)(
                    session,
                    std::ptr::null_mut(),
                    input_names.as_ptr(),
                    input_values.as_ptr() as *const *const c_void,
                    input_values.len(),
                    output_names.as_ptr(),
                    outputs.as_mut_ptr(),
                    outputs.len(),
                )
            };
            self.check_status(status)?;
            Ok(outputs)
        }

        pub fn tensor_data_f32_len(
            &self,
            value: *mut c_void,
            elements: usize,
        ) -> Result<Vec<f32>, OrtSysError> {
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let st = unsafe {
                self.slot_fn::<FnGetTensorMutableData>(idx::GETTENSORMUTABLEDATA)(value, &mut ptr)
            };
            self.check_status(st)?;
            // SAFETY: ORT 输出 float32 张量，元素数由输出形状给出
            let data = unsafe { std::slice::from_raw_parts(ptr as *const f32, elements) };
            Ok(data.to_vec())
        }

        /// 输出张量元素数（GetTensorTypeAndShape → GetTensorShapeElementCount）。
        pub fn output_elements(&self, value: *mut c_void) -> Result<usize, OrtSysError> {
            let shape = unsafe {
                self.slot_fn::<FnGetTensorTypeAndShape>(idx::GETTENSORTYPEANDSHAPE)(value)
            };
            let n = unsafe {
                self.slot_fn::<FnGetTensorShapeElementCount>(idx::GETTENSORSHAPEELEMENTCOUNT)(shape)
            };
            Ok(n)
        }

        pub fn release_session(&self, session: *mut c_void) {
            unsafe { self.slot_fn::<FnReleaseSession>(idx::RELEASESESSION)(session) };
        }

        pub fn release_env(&self, env: *mut c_void) {
            unsafe { self.slot_fn::<FnReleaseEnv>(idx::RELEASEENV)(env) };
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(not(target_os = "windows"))]
pub mod stub_impl {
    //! 板端阶段启用（dlopen + libc feature）；当前非 Windows 目标一律 Unsupported。
    use std::path::Path;
    use thiserror::Error;

    #[derive(Error, Debug, Clone)]
    pub enum OrtSysError {
        #[error("onnxruntime.dll 加载失败: {0}")]
        Load(String),
        #[error("OrtGetApiBase/GetApi 失败 (v{version})")]
        GetApi { version: u32 },
        #[error("ORT 调用失败: {msg}")]
        Status { msg: String },
        #[error("ORT I/O 不符: {0}")]
        Shape(String),
    }

    impl OrtSysError {
        pub fn is_unavailable(&self) -> bool {
            true
        }
    }

    pub struct Ort;

    impl Ort {
        pub fn load(_dll_path: &Path) -> Result<Self, OrtSysError> {
            Err(OrtSysError::Load(
                "非 Windows 目标：dlopen 路线待板端阶段启用".into(),
            ))
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub_impl::*;
