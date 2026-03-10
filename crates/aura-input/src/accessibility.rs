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
