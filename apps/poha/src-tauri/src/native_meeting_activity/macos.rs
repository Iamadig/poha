use std::ffi::{CStr, c_char, c_void};
use std::mem::size_of;
use std::ptr;

use objc2_app_kit::NSRunningApplication;

use super::{ActivitySource, NativeActivityCollectionError, ProcessActivity};

pub(super) struct MacosActivitySource;

impl ActivitySource for MacosActivitySource {
    fn snapshot(&self) -> Result<Vec<ProcessActivity>, NativeActivityCollectionError> {
        let audio = collect_core_audio_activity();
        let power = collect_power_assertion_activity();

        match (audio, power) {
            (Ok(mut audio), Ok(power)) => {
                audio.extend(power);
                Ok(audio)
            }
            (Ok(audio), Err(_)) => Ok(audio),
            (Err(_), Ok(power)) => Ok(power),
            (Err(_), Err(_)) => Err(NativeActivityCollectionError::unavailable(
                "CoreAudio and IOKit meeting activity sources",
            )),
        }
    }
}

type AudioObjectId = u32;
type AudioObjectPropertySelector = u32;
type AudioObjectPropertyScope = u32;
type AudioObjectPropertyElement = u32;
type OsStatus = i32;

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    element: AudioObjectPropertyElement,
}

const AUDIO_SYSTEM_OBJECT: AudioObjectId = 1;
const AUDIO_PROCESS_OBJECT_LIST: u32 = fourcc(*b"prs#");
const AUDIO_PROCESS_BUNDLE_ID: u32 = fourcc(*b"pbid");
const AUDIO_PROCESS_IS_RUNNING_INPUT: u32 = fourcc(*b"piri");
const AUDIO_PROCESS_IS_RUNNING_OUTPUT: u32 = fourcc(*b"piro");
const AUDIO_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
const AUDIO_ELEMENT_MAIN: u32 = 0;
const MAX_AUDIO_PROCESS_LIST_BYTES: u32 = 1024 * 1024;

const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

fn audio_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        selector,
        scope: AUDIO_SCOPE_GLOBAL,
        element: AUDIO_ELEMENT_MAIN,
    }
}

fn collect_core_audio_activity() -> Result<Vec<ProcessActivity>, NativeActivityCollectionError> {
    let process_ids = audio_process_object_ids()?;
    let mut activity = Vec::new();

    for process_id in process_ids {
        // Process objects can disappear between the list and property reads.
        // Treat an individual race as no evidence, not as a global failure.
        let audio_input_active = audio_u32_property(
            process_id,
            AUDIO_PROCESS_IS_RUNNING_INPUT,
            "CoreAudio input activity",
        )
        .is_ok_and(|value| value != 0);
        let audio_output_active = audio_u32_property(
            process_id,
            AUDIO_PROCESS_IS_RUNNING_OUTPUT,
            "CoreAudio output activity",
        )
        .is_ok_and(|value| value != 0);
        if !audio_input_active && !audio_output_active {
            continue;
        }

        let Ok(bundle_id) = audio_bundle_id(process_id) else {
            continue;
        };
        if bundle_id.trim().is_empty() {
            continue;
        }

        activity.push(ProcessActivity {
            bundle_id,
            audio_input_active,
            audio_output_active,
            qualifying_power_assertion: false,
        });
    }

    Ok(activity)
}

fn audio_process_object_ids() -> Result<Vec<AudioObjectId>, NativeActivityCollectionError> {
    let address = audio_address(AUDIO_PROCESS_OBJECT_LIST);
    let mut data_size = 0_u32;
    // SAFETY: `address` and `data_size` are valid for the duration of the
    // call, and this read-only property has no qualifier data.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            AUDIO_SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut data_size,
        )
    };
    check_status("CoreAudio process list size", status)?;
    if data_size == 0 {
        return Ok(Vec::new());
    }
    if data_size > MAX_AUDIO_PROCESS_LIST_BYTES
        || data_size as usize % size_of::<AudioObjectId>() != 0
    {
        return Err(NativeActivityCollectionError::unavailable(
            "CoreAudio process list shape",
        ));
    }

    let mut process_ids = vec![0_u32; data_size as usize / size_of::<AudioObjectId>()];
    let capacity_size = data_size;
    // SAFETY: the vector has exactly `data_size` initialized writable bytes,
    // and both address/data-size pointers remain valid for the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            AUDIO_SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut data_size,
            process_ids.as_mut_ptr().cast(),
        )
    };
    check_status("CoreAudio process list", status)?;
    if data_size > capacity_size || data_size as usize % size_of::<AudioObjectId>() != 0 {
        return Err(NativeActivityCollectionError::unavailable(
            "CoreAudio process list shape",
        ));
    }
    process_ids.truncate(data_size as usize / size_of::<AudioObjectId>());
    Ok(process_ids)
}

fn audio_u32_property(
    object_id: AudioObjectId,
    selector: u32,
    component: &'static str,
) -> Result<u32, NativeActivityCollectionError> {
    let address = audio_address(selector);
    let mut value = 0_u32;
    let mut data_size = size_of::<u32>() as u32;
    // SAFETY: `value` is an initialized `u32` with matching data size and the
    // property address is valid for the duration of this read-only call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object_id,
            &address,
            0,
            ptr::null(),
            &mut data_size,
            (&mut value as *mut u32).cast(),
        )
    };
    check_status(component, status)?;
    if data_size as usize != size_of::<u32>() {
        return Err(NativeActivityCollectionError::unavailable(component));
    }
    Ok(value)
}

fn audio_bundle_id(object_id: AudioObjectId) -> Result<String, NativeActivityCollectionError> {
    let address = audio_address(AUDIO_PROCESS_BUNDLE_ID);
    let mut string: CfStringRef = ptr::null();
    let mut data_size = size_of::<CfStringRef>() as u32;
    // SAFETY: `string` is a pointer-sized output buffer. CoreAudio documents
    // this property as a caller-owned CFString, released below by `OwnedCf`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object_id,
            &address,
            0,
            ptr::null(),
            &mut data_size,
            (&mut string as *mut CfStringRef).cast(),
        )
    };
    check_status("CoreAudio process bundle identity", status)?;
    if data_size as usize != size_of::<CfStringRef>() || string.is_null() {
        return Err(NativeActivityCollectionError::unavailable(
            "CoreAudio process bundle identity",
        ));
    }

    // SAFETY: a successful bundle-ID property read returns a retained object
    // owned by the caller according to AudioHardware.h.
    let string = unsafe { OwnedCf::from_raw(string.cast()) }.ok_or_else(|| {
        NativeActivityCollectionError::unavailable("CoreAudio process bundle identity")
    })?;
    cf_string_to_string(string.as_ptr().cast()).ok_or_else(|| {
        NativeActivityCollectionError::unavailable("CoreAudio process bundle identity")
    })
}

fn check_status(
    component: &'static str,
    status: OsStatus,
) -> Result<(), NativeActivityCollectionError> {
    if status == 0 {
        Ok(())
    } else {
        Err(NativeActivityCollectionError::os_status(component, status))
    }
}

type IoReturn = i32;
type CfTypeRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfArrayRef = *const c_void;
type CfNumberRef = *const c_void;
type CfStringRef = *const c_void;
type CfTypeId = usize;
type CfIndex = isize;

const CF_NUMBER_SINT32_TYPE: CfIndex = 3;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const MAX_CF_STRING_BYTES: CfIndex = 4096;

fn collect_power_assertion_activity() -> Result<Vec<ProcessActivity>, NativeActivityCollectionError>
{
    let mut assertions: CfDictionaryRef = ptr::null();
    // SAFETY: `assertions` is a valid out-pointer. On success IOKit transfers
    // ownership of a CFDictionary to the caller.
    let status = unsafe { IOPMCopyAssertionsByProcess(&mut assertions) };
    if status != 0 {
        return Err(NativeActivityCollectionError::os_status(
            "IOKit power assertions",
            status,
        ));
    }
    // SAFETY: success returns a retained dictionary, guarded against null.
    let assertions = unsafe { OwnedCf::from_raw(assertions.cast()) }.ok_or_else(|| {
        NativeActivityCollectionError::unavailable("IOKit power assertion dictionary")
    })?;
    if !cf_has_type(assertions.as_ptr(), unsafe { CFDictionaryGetTypeID() }) {
        return Err(NativeActivityCollectionError::unavailable(
            "IOKit power assertion dictionary",
        ));
    }

    let type_key = cf_string_literal(b"AssertType\0")?;
    let level_key = cf_string_literal(b"AssertLevel\0")?;
    let count = unsafe { CFDictionaryGetCount(assertions.as_ptr().cast()) };
    if count < 0 || count > 100_000 {
        return Err(NativeActivityCollectionError::unavailable(
            "IOKit power assertion dictionary shape",
        ));
    }
    let mut keys = vec![ptr::null(); count as usize];
    let mut values = vec![ptr::null(); count as usize];
    // SAFETY: both vectors have capacity for exactly the dictionary count and
    // CoreFoundation writes borrowed key/value pointers only.
    unsafe {
        CFDictionaryGetKeysAndValues(
            assertions.as_ptr().cast(),
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
        )
    };

    let mut activity = Vec::new();
    for (pid_value, assertion_array) in keys.into_iter().zip(values) {
        let Some(pid) = cf_number_i32(pid_value.cast()) else {
            continue;
        };
        if pid <= 0
            || !assertion_array_has_qualifying_activity(
                assertion_array.cast(),
                type_key.as_ptr().cast(),
                level_key.as_ptr().cast(),
            )
        {
            continue;
        }
        let Some(bundle_id) = bundle_id_for_pid(pid) else {
            continue;
        };

        activity.push(ProcessActivity {
            bundle_id,
            audio_input_active: false,
            audio_output_active: false,
            qualifying_power_assertion: true,
        });
    }

    Ok(activity)
}

fn assertion_array_has_qualifying_activity(
    assertions: CfArrayRef,
    type_key: CfStringRef,
    level_key: CfStringRef,
) -> bool {
    if assertions.is_null() || !cf_has_type(assertions.cast(), unsafe { CFArrayGetTypeID() }) {
        return false;
    }
    let count = unsafe { CFArrayGetCount(assertions) };
    if count <= 0 || count > 100_000 {
        return false;
    }

    for index in 0..count {
        let assertion = unsafe { CFArrayGetValueAtIndex(assertions, index) };
        if assertion.is_null() || !cf_has_type(assertion.cast(), unsafe { CFDictionaryGetTypeID() })
        {
            continue;
        }
        let level = unsafe { CFDictionaryGetValue(assertion.cast(), level_key.cast()) };
        if cf_number_i32(level.cast()).is_none_or(|level| level <= 0) {
            continue;
        }
        let assertion_type = unsafe { CFDictionaryGetValue(assertion.cast(), type_key.cast()) };
        let Some(assertion_type) = cf_string_to_string(assertion_type.cast()) else {
            continue;
        };
        if qualifying_power_assertion_type(&assertion_type) {
            return true;
        }
    }
    false
}

fn qualifying_power_assertion_type(assertion_type: &str) -> bool {
    matches!(
        assertion_type,
        "PreventUserIdleSystemSleep"
            | "PreventUserIdleDisplaySleep"
            | "PreventSystemSleep"
            | "NoDisplaySleepAssertion"
    )
}

fn bundle_id_for_pid(pid: i32) -> Option<String> {
    let application = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let bundle_id = application.bundleIdentifier()?.to_string();
    (!bundle_id.trim().is_empty()).then_some(bundle_id)
}

fn cf_number_i32(number: CfNumberRef) -> Option<i32> {
    if number.is_null() || !cf_has_type(number.cast(), unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut value = 0_i32;
    // SAFETY: the runtime type has been checked and `value` is a correctly
    // sized destination for `kCFNumberSInt32Type` conversion.
    let converted = unsafe {
        CFNumberGetValue(
            number,
            CF_NUMBER_SINT32_TYPE,
            (&mut value as *mut i32).cast(),
        )
    };
    (converted != 0).then_some(value)
}

fn cf_string_literal(value: &'static [u8]) -> Result<OwnedCf, NativeActivityCollectionError> {
    debug_assert_eq!(value.last(), Some(&0));
    // SAFETY: callers provide a static NUL-terminated byte string and null
    // allocator means the default CoreFoundation allocator.
    let string = unsafe {
        CFStringCreateWithCString(
            ptr::null(),
            value.as_ptr().cast::<c_char>(),
            CF_STRING_ENCODING_UTF8,
        )
    };
    // SAFETY: CFStringCreate follows the Create Rule and returns owned +1.
    unsafe { OwnedCf::from_raw(string.cast()) }
        .ok_or_else(|| NativeActivityCollectionError::unavailable("CoreFoundation string creation"))
}

fn cf_string_to_string(string: CfStringRef) -> Option<String> {
    if string.is_null() || !cf_has_type(string.cast(), unsafe { CFStringGetTypeID() }) {
        return None;
    }
    let length = unsafe { CFStringGetLength(string) };
    if length < 0 {
        return None;
    }
    let maximum = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) };
    if maximum < 0 || maximum >= MAX_CF_STRING_BYTES {
        return None;
    }
    let buffer_size = maximum.checked_add(1)?;
    let mut buffer = vec![0_u8; buffer_size as usize];
    // SAFETY: `buffer` is writable for `buffer_size`, and the input has been
    // runtime-checked as a CFString.
    let converted = unsafe {
        CFStringGetCString(
            string,
            buffer.as_mut_ptr().cast::<c_char>(),
            buffer_size,
            CF_STRING_ENCODING_UTF8,
        )
    };
    if converted == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString guarantees a NUL terminator on success.
    let value = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) };
    value.to_str().ok().map(ToOwned::to_owned)
}

fn cf_has_type(value: CfTypeRef, expected: CfTypeId) -> bool {
    !value.is_null() && unsafe { CFGetTypeID(value) } == expected
}

struct OwnedCf(CfTypeRef);

impl OwnedCf {
    unsafe fn from_raw(value: CfTypeRef) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }

    fn as_ptr(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: `OwnedCf` is constructed only from +1 Create/Copy results and
        // releases its non-null value exactly once.
        unsafe { CFRelease(self.0) };
    }
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object_id: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        data_size: *mut u32,
    ) -> OsStatus;

    fn AudioObjectGetPropertyData(
        object_id: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_data_size: u32,
        qualifier_data: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMCopyAssertionsByProcess(assertions_by_pid: *mut CfDictionaryRef) -> IoReturn;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CfTypeRef);
    fn CFGetTypeID(value: CfTypeRef) -> CfTypeId;

    fn CFDictionaryGetTypeID() -> CfTypeId;
    fn CFDictionaryGetCount(dictionary: CfDictionaryRef) -> CfIndex;
    fn CFDictionaryGetKeysAndValues(
        dictionary: CfDictionaryRef,
        keys: *mut CfTypeRef,
        values: *mut CfTypeRef,
    );
    fn CFDictionaryGetValue(dictionary: CfDictionaryRef, key: *const c_void) -> CfTypeRef;

    fn CFArrayGetTypeID() -> CfTypeId;
    fn CFArrayGetCount(array: CfArrayRef) -> CfIndex;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, index: CfIndex) -> CfTypeRef;

    fn CFNumberGetTypeID() -> CfTypeId;
    fn CFNumberGetValue(number: CfNumberRef, number_type: CfIndex, value: *mut c_void) -> u8;

    fn CFStringGetTypeID() -> CfTypeId;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        string: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetLength(string: CfStringRef) -> CfIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CfIndex, encoding: u32) -> CfIndex;
    fn CFStringGetCString(
        string: CfStringRef,
        buffer: *mut c_char,
        buffer_size: CfIndex,
        encoding: u32,
    ) -> u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_audio_selectors_match_apple_fourcc_values() {
        assert_eq!(AUDIO_PROCESS_OBJECT_LIST, 0x7072_7323);
        assert_eq!(AUDIO_PROCESS_IS_RUNNING_INPUT, 0x7069_7269);
        assert_eq!(AUDIO_PROCESS_IS_RUNNING_OUTPUT, 0x7069_726f);
    }

    #[test]
    fn only_call_correlated_power_assertions_qualify() {
        for assertion_type in [
            "PreventUserIdleSystemSleep",
            "PreventUserIdleDisplaySleep",
            "PreventSystemSleep",
            "NoDisplaySleepAssertion",
        ] {
            assert!(qualifying_power_assertion_type(assertion_type));
        }
        for assertion_type in ["UserIsActive", "NetworkClientActive", "BackgroundTask"] {
            assert!(!qualifying_power_assertion_type(assertion_type));
        }
    }

    #[test]
    fn native_sources_can_be_polled_without_special_ui_permissions() {
        // The value depends on live machine state; the assertion here is that
        // public CoreAudio/IOKit/AppKit APIs can be traversed safely.
        let _ = MacosActivitySource.snapshot();
    }
}
