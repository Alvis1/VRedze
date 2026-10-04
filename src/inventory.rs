use ash::{Entry, vk};
use serde_json::{Value, json};
use std::{ffi::CStr, fs};

pub fn collect() -> Value {
    json!({
        "schema_version": 1,
        "kind": "inventory_only",
        "frame_hardware_decoding_verified": false,
        "architecture": std::env::consts::ARCH,
        "os_release": fs::read_to_string("/etc/os-release").ok(),
        "device_tree_model": fs::read_to_string("/sys/firmware/devicetree/base/model")
            .ok().map(|s| s.trim_end_matches('\0').to_owned()),
        "vulkan": vulkan().unwrap_or_else(|e| json!({"error": e.to_string()})),
        "openxr": openxr().unwrap_or_else(|e| json!({"error": e.to_string()})),
        "next_gate": "Real H.264, HEVC and AV1 decode, then GPU import and OpenXR presentation on Frame"
    })
}

fn vulkan() -> anyhow::Result<Value> {
    // SAFETY: The Vulkan loader owns function pointers for the lifetime of Entry.
    let entry = unsafe { Entry::load()? };
    let app = vk::ApplicationInfo::default()
        .application_name(c"Just Video probe")
        .api_version(vk::API_VERSION_1_1);
    // No surface, logical device, or active XR session is created.
    let instance = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )?
    };
    let result = inspect_devices(&instance);
    unsafe { instance.destroy_instance(None) };
    result
}

fn inspect_devices(instance: &ash::Instance) -> anyhow::Result<Value> {
    let mut devices = Vec::new();
    for device in unsafe { instance.enumerate_physical_devices()? } {
        let properties = unsafe { instance.get_physical_device_properties(device) };
        let extensions = unsafe { instance.enumerate_device_extension_properties(device)? };
        let names: Vec<String> = extensions
            .iter()
            .map(|e| {
                unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let queues = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let mut relevant: Vec<_> = names
            .iter()
            .filter(|name| {
                name.contains("video")
                    || name.contains("external_memory")
                    || name.contains("external_semaphore")
                    || name.contains("drm_format_modifier")
                    || name.contains("sampler_ycbcr")
            })
            .cloned()
            .collect();
        relevant.sort();
        devices.push(json!({
            "name": unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }.to_string_lossy(),
            "vendor_id": properties.vendor_id,
            "device_id": properties.device_id,
            "device_type": format!("{:?}", properties.device_type),
            "api_version": format!("{}.{}.{}", vk::api_version_major(properties.api_version),
                vk::api_version_minor(properties.api_version), vk::api_version_patch(properties.api_version)),
            "driver_version_raw": properties.driver_version,
            "relevant_extensions": relevant,
            "decode_queue_families": queues.iter().enumerate()
                .filter(|(_, q)| q.queue_flags.contains(vk::QueueFlags::VIDEO_DECODE_KHR))
                .map(|(i, q)| json!({"index": i, "count": q.queue_count})).collect::<Vec<_>>(),
            "note": "Extensions are advertised capabilities, not a tested codec/profile or zero-copy result"
        }));
    }
    Ok(json!({"devices": devices}))
}

fn openxr() -> anyhow::Result<Value> {
    let entry = crate::xr::runtime::entry()?;
    let extensions = entry.enumerate_extensions()?;
    Ok(json!({
        "loader_available": true,
        "khr_vulkan_enable": extensions.khr_vulkan_enable,
        "khr_vulkan_enable2": extensions.khr_vulkan_enable2,
        "note": "Runtime extensions only; HMD session and device selection remain untested"
    }))
}
