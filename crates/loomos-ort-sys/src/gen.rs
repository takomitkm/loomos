// 由 scripts/gen_ort_sys.py 生成（onnxruntime_c_api.h v20）——勿手改
pub const ORT_API_VERSION: u32 = 20;
pub const ORT_API_FIELD_COUNT: usize = 285;

/// 所需函数在 OrtApi 表中的槽位索引（字段序即头文件声明序）
pub mod idx {
    pub const CREATEENV: usize = 3;
    pub const RELEASEENV: usize = 92;
    pub const CREATESESSION: usize = 7;
    pub const RELEASESESSION: usize = 95;
    pub const CREATESESSIONOPTIONS: usize = 10;
    pub const RELEASESESSIONOPTIONS: usize = 100;
    pub const SETINTRAOPNUMTHREADS: usize = 24;
    pub const SETSESSIONGRAPHOPTIMIZATIONLEVEL: usize = 23;
    pub const GETERRORMESSAGE: usize = 2;
    pub const RELEASESTATUS: usize = 93;
    pub const SESSIONGETINPUTCOUNT: usize = 30;
    pub const SESSIONGETINPUTNAME: usize = 36;
    pub const SESSIONGETOUTPUTCOUNT: usize = 31;
    pub const SESSIONGETOUTPUTNAME: usize = 37;
    pub const GETALLOCATORWITHDEFAULTOPTIONS: usize = 78;
    pub const SESSIONGETINPUTTYPEINFO: usize = 33;
    pub const SESSIONGETOUTPUTTYPEINFO: usize = 34;
    pub const CASTTYPEINFOTOTENSORINFO: usize = 55;
    pub const GETDIMENSIONSCOUNT: usize = 61;
    pub const GETDIMENSIONS: usize = 62;
    pub const GETTENSORELEMENTTYPE: usize = 60;
    pub const GETTENSORTYPEANDSHAPE: usize = 65;
    pub const GETTENSORSHAPEELEMENTCOUNT: usize = 64;
    pub const CREATECPUMEMORYINFO: usize = 69;
    pub const RELEASEMEMORYINFO: usize = 94;
    pub const CREATETENSORWITHDATAASORTVALUE: usize = 49;
    pub const RUN: usize = 9;
    pub const GETTENSORMUTABLEDATA: usize = 51;
    pub const RELEASEVALUE: usize = 96;
}

/// OrtApi 表：v20 全部为函数指针（64 位平台每项 8 字节）
#[repr(C)]
pub struct OrtApi {
    pub slots: [usize; ORT_API_FIELD_COUNT],
}

/// OrtAllocator 结构（首字段 version，随后函数指针）
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct OrtAllocator {
    pub version: u32,
    pub Alloc: usize,
    pub Free: usize,
    pub Info: usize,
    pub Reserve: usize,
}
