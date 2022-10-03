pub const PLCTAG_STATUS_PENDING: i32 = 1;
pub const PLCTAG_STATUS_OK: i32 = 0;
pub const PLCTAG_ERR_ABORT: i32 = -1;
pub const PLCTAG_ERR_BAD_CONFIG: i32 = -2;
pub const PLCTAG_ERR_BAD_CONNECTION: i32 = -3;
pub const PLCTAG_ERR_BAD_DATA: i32 = -4;
pub const PLCTAG_ERR_BAD_DEVICE: i32 = -5;
pub const PLCTAG_ERR_BAD_GATEWAY: i32 = -6;
pub const PLCTAG_ERR_BAD_PARAM: i32 = -7;
pub const PLCTAG_ERR_BAD_REPLY: i32 = -8;
pub const PLCTAG_ERR_BAD_STATUS: i32 = -9;
pub const PLCTAG_ERR_CLOSE: i32 = -10;
pub const PLCTAG_ERR_CREATE: i32 = -11;
pub const PLCTAG_ERR_DUPLICATE: i32 = -12;
pub const PLCTAG_ERR_ENCODE: i32 = -13;
pub const PLCTAG_ERR_MUTEX_DESTROY: i32 = -14;
pub const PLCTAG_ERR_MUTEX_INIT: i32 = -15;
pub const PLCTAG_ERR_MUTEX_LOCK: i32 = -16;
pub const PLCTAG_ERR_MUTEX_UNLOCK: i32 = -17;
pub const PLCTAG_ERR_NOT_ALLOWED: i32 = -18;
pub const PLCTAG_ERR_NOT_FOUND: i32 = -19;
pub const PLCTAG_ERR_NOT_IMPLEMENTED: i32 = -20;
pub const PLCTAG_ERR_NO_DATA: i32 = -21;
pub const PLCTAG_ERR_NO_MATCH: i32 = -22;
pub const PLCTAG_ERR_NO_MEM: i32 = -23;
pub const PLCTAG_ERR_NO_RESOURCES: i32 = -24;
pub const PLCTAG_ERR_NULL_PTR: i32 = -25;
pub const PLCTAG_ERR_OPEN: i32 = -26;
pub const PLCTAG_ERR_OUT_OF_BOUNDS: i32 = -27;
pub const PLCTAG_ERR_READ: i32 = -28;
pub const PLCTAG_ERR_REMOTE_ERR: i32 = -29;
pub const PLCTAG_ERR_THREAD_CREATE: i32 = -30;
pub const PLCTAG_ERR_THREAD_JOIN: i32 = -31;
pub const PLCTAG_ERR_TIMEOUT: i32 = -32;
pub const PLCTAG_ERR_TOO_LARGE: i32 = -33;
pub const PLCTAG_ERR_TOO_SMALL: i32 = -34;
pub const PLCTAG_ERR_UNSUPPORTED: i32 = -35;
pub const PLCTAG_ERR_WINSOCK: i32 = -36;
pub const PLCTAG_ERR_WRITE: i32 = -37;
pub const PLCTAG_ERR_PARTIAL: i32 = -38;
pub const PLCTAG_ERR_BUSY: i32 = -39;
pub const PLCTAG_DEBUG_NONE: u32 = 0;
pub const PLCTAG_DEBUG_ERROR: u32 = 1;
pub const PLCTAG_DEBUG_WARN: u32 = 2;
pub const PLCTAG_DEBUG_INFO: u32 = 3;
pub const PLCTAG_DEBUG_DETAIL: u32 = 4;
pub const PLCTAG_DEBUG_SPEW: u32 = 5;
pub const PLCTAG_EVENT_READ_STARTED: u32 = 1;
pub const PLCTAG_EVENT_READ_COMPLETED: u32 = 2;
pub const PLCTAG_EVENT_WRITE_STARTED: u32 = 3;
pub const PLCTAG_EVENT_WRITE_COMPLETED: u32 = 4;
pub const PLCTAG_EVENT_ABORTED: u32 = 5;
pub const PLCTAG_EVENT_DESTROYED: u32 = 6;

extern "C" {
    pub fn plc_tag_decode_error(err: ::std::os::raw::c_int) -> *const ::std::os::raw::c_char;
}
extern "C" {
    pub fn plc_tag_set_debug_level(debug_level: ::std::os::raw::c_int);
}
extern "C" {
    pub fn plc_tag_check_lib_version(
        req_major: ::std::os::raw::c_int,
        req_minor: ::std::os::raw::c_int,
        req_patch: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_create(
        attrib_str: *const ::std::os::raw::c_char,
        timeout: ::std::os::raw::c_int,
    ) -> i32;
}
extern "C" {
    pub fn plc_tag_shutdown();
}
extern "C" {
    pub fn plc_tag_register_callback(
        tag_id: i32,
        tag_callback_func: ::std::option::Option<
            unsafe extern "C" fn(
                tag_id: i32,
                event: ::std::os::raw::c_int,
                status: ::std::os::raw::c_int,
            ),
        >,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_unregister_callback(tag_id: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_register_logger(
        log_callback_func: ::std::option::Option<
            unsafe extern "C" fn(
                tag_id: i32,
                debug_level: ::std::os::raw::c_int,
                message: *const ::std::os::raw::c_char,
            ),
        >,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_unregister_logger() -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_lock(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_unlock(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_abort(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_destroy(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_read(tag: i32, timeout: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_status(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_write(tag: i32, timeout: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_int_attribute(
        tag: i32,
        attrib_name: *const ::std::os::raw::c_char,
        default_value: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_set_int_attribute(
        tag: i32,
        attrib_name: *const ::std::os::raw::c_char,
        new_value: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_size(tag: i32) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_bit(tag: i32, offset_bit: ::std::os::raw::c_int) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_set_bit(
        tag: i32,
        offset_bit: ::std::os::raw::c_int,
        val: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_uint64(tag: i32, offset: ::std::os::raw::c_int) -> u64;
}
extern "C" {
    pub fn plc_tag_set_uint64(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: u64,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_int64(tag: i32, offset: ::std::os::raw::c_int) -> i64;
}
extern "C" {
    pub fn plc_tag_set_int64(
        arg1: i32,
        offset: ::std::os::raw::c_int,
        val: i64,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_uint32(tag: i32, offset: ::std::os::raw::c_int) -> u32;
}
extern "C" {
    pub fn plc_tag_set_uint32(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: u32,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_int32(tag: i32, offset: ::std::os::raw::c_int) -> i32;
}
extern "C" {
    pub fn plc_tag_set_int32(
        arg1: i32,
        offset: ::std::os::raw::c_int,
        val: i32,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_uint16(tag: i32, offset: ::std::os::raw::c_int) -> u16;
}
extern "C" {
    pub fn plc_tag_set_uint16(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: u16,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_int16(tag: i32, offset: ::std::os::raw::c_int) -> i16;
}
extern "C" {
    pub fn plc_tag_set_int16(
        arg1: i32,
        offset: ::std::os::raw::c_int,
        val: i16,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_uint8(tag: i32, offset: ::std::os::raw::c_int) -> u8;
}
extern "C" {
    pub fn plc_tag_set_uint8(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: u8,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_int8(tag: i32, offset: ::std::os::raw::c_int) -> i8;
}
extern "C" {
    pub fn plc_tag_set_int8(
        arg1: i32,
        offset: ::std::os::raw::c_int,
        val: i8,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_float64(tag: i32, offset: ::std::os::raw::c_int) -> f64;
}
extern "C" {
    pub fn plc_tag_set_float64(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: f64,
    ) -> ::std::os::raw::c_int;
}
extern "C" {
    pub fn plc_tag_get_float32(tag: i32, offset: ::std::os::raw::c_int) -> f32;
}
extern "C" {
    pub fn plc_tag_set_float32(
        tag: i32,
        offset: ::std::os::raw::c_int,
        val: f32,
    ) -> ::std::os::raw::c_int;
}
