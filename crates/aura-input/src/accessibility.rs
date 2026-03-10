use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::base::CFTypeRef) -> bool;
}

/// Check if the process has Accessibility permission.
/// If `prompt` is true and permission is not granted, opens System Settings.
pub fn check_accessibility(prompt: bool) -> bool {
    // SAFETY: AXIsProcessTrustedWithOptions is a macOS Accessibility C API that accepts
    // a CFDictionary options parameter. We construct a valid CFDictionary with the
    // "AXTrustedCheckOptionPrompt" key and a CFBoolean value. The dictionary remains
    // valid for the duration of the FFI call. The function returns a plain bool.
    unsafe {
        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = if prompt {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef())
    }
}
