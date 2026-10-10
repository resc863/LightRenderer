#[repr(transparent)]
pub struct SdkPath(*const u8);
// An immutable pointer to a process-lifetime null-terminated string.
unsafe impl Sync for SdkPath {}
#[unsafe(no_mangle)]
pub static D3D12SDKVersion: u32 = 619;
#[unsafe(no_mangle)]
pub static D3D12SDKPath: SdkPath = SdkPath(c".\\D3D12\\".as_ptr().cast());
