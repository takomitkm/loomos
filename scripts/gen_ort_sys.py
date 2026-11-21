#!/usr/bin/env python
"""从 onnxruntime_c_api.h 生成 loomooms-ort-sys 绑定（OrtApi 函数指针槽位索引 + OrtAllocator 字段）。

用法：python scripts/gen_ort_sys.py <onnxruntime_c_api.h> > crates/loomos-ort-sys/src/gen.rs
"""
import re
import sys

ORT_API_BLOCK = re.compile(r"struct OrtApi\s*\{(.*?)\n\};", re.S)
ORT_ALLOCATOR_BLOCK = re.compile(r"struct OrtAllocator\s*\{(.*?)\} OrtAllocator;", re.S)
MACRO_FIELD = re.compile(r"ORT_API2_STATUS\(\s*(\w+)\s*,")
DIRECT_FIELD = re.compile(r"ORT_API_CALL\*\s*(\w+)\)")
API_FIELD = re.compile(r"ORT_API\(\s*[^,()]+,\s*(\w+)\s*,")
RELEASE_FIELD = re.compile(r"ORT_CLASS_RELEASE\((\w+)\)")

COMBINED = re.compile(
    MACRO_FIELD.pattern + "|" + DIRECT_FIELD.pattern + "|" +
    API_FIELD.pattern + "|" + RELEASE_FIELD.pattern
)


def parse_ort_api_fields(header):
    m = ORT_API_BLOCK.search(header)
    if not m:
        raise SystemExit("找不到 struct OrtApi")
    body = m.group(1)
    ordered = []
    for mm in COMBINED.finditer(body):
        # ORT_CLASS_RELEASE(X) 的字段名是 Release+X（token pasting），且位置就在声明处
        name = mm.group(1) or mm.group(2) or mm.group(3)
        if mm.group(4):
            name = "Release" + mm.group(4)
        if name and name not in ordered:
            ordered.append(name)
    return ordered


def parse_ort_allocator(header):
    m = ORT_ALLOCATOR_BLOCK.search(header)
    if not m:
        raise SystemExit("找不到 struct OrtAllocator")
    body = m.group(1)
    out = []
    for line in body.splitlines():
        line = line.strip()
        mm = re.match(r"uint32_t\s+(\w+);", line)
        if mm:
            out.append(("u32", mm.group(1)))
            continue
        mm = re.search(r"ORT_API_CALL\*\s*(\w+)", line)
        if mm:
            out.append(("ptr", mm.group(1)))
    return out


def main():
    header = open(sys.argv[1], encoding="utf-8", errors="replace").read()
    version = re.search(r"#define ORT_API_VERSION\s+(\d+)", header).group(1)
    fields = parse_ort_api_fields(header)
    alloc = parse_ort_allocator(header)

    need = [
        "CreateEnv", "ReleaseEnv",
        "CreateSession", "ReleaseSession",
        "CreateSessionOptions", "ReleaseSessionOptions",
        "SetIntraOpNumThreads", "SetSessionGraphOptimizationLevel",
        "GetErrorMessage", "ReleaseStatus",
        "SessionGetInputCount", "SessionGetInputName",
        "SessionGetOutputCount", "SessionGetOutputName",
        "GetAllocatorWithDefaultOptions",
        "SessionGetInputTypeInfo", "SessionGetOutputTypeInfo",
        "CastTypeInfoToTensorInfo",
        "GetDimensionsCount", "GetDimensions", "GetTensorElementType",
        "GetTensorTypeAndShape", "GetTensorShapeElementCount",
        "CreateCpuMemoryInfo", "ReleaseMemoryInfo",
        "CreateTensorWithDataAsOrtValue",
        "Run",
        "GetTensorMutableData",
        "ReleaseValue",
    ]
    missing = [n for n in need if n not in fields]
    if missing:
        raise SystemExit("OrtApi 缺少需要的函数: %s" % missing)

    out = []
    out.append("// 由 scripts/gen_ort_sys.py 生成（onnxruntime_c_api.h v%s）——勿手改" % version)
    out.append("pub const ORT_API_VERSION: u32 = %s;" % version)
    out.append("pub const ORT_API_FIELD_COUNT: usize = %d;" % len(fields))
    out.append("")
    out.append("/// 所需函数在 OrtApi 表中的槽位索引（字段序即头文件声明序）")
    out.append("pub mod idx {")
    for n in need:
        out.append("    pub const %s: usize = %d;" % (n.upper(), fields.index(n)))
    out.append("}")
    out.append("")
    out.append("/// OrtApi 表：v20 全部为函数指针（64 位平台每项 8 字节）")
    out.append("#[repr(C)]")
    out.append("pub struct OrtApi {")
    out.append("    pub slots: [usize; ORT_API_FIELD_COUNT],")
    out.append("}")
    out.append("")
    out.append("/// OrtAllocator 结构（首字段 version，随后函数指针）")
    out.append("#[repr(C)]")
    out.append("#[derive(Clone, Copy)]")
    out.append("#[allow(non_snake_case)]")
    out.append("pub struct OrtAllocator {")
    for kind, name in alloc:
        if kind == "u32":
            out.append("    pub %s: u32," % name)
        else:
            out.append("    pub %s: usize," % name)
    out.append("}")
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
