//! Small, local application icons for the dashboard, independent of the daemon.
use std::collections::HashMap;

#[tauri::command]
pub async fn app_icons(
    app: tauri::AppHandle,
    names: Vec<String>,
) -> Result<HashMap<String, String>, String> {
    #[cfg(target_os = "macos")]
    {
        let (send, receive) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let icons = objc2::rc::autoreleasepool(|_| {
                names
                    .into_iter()
                    .take(64)
                    .filter_map(|name| macos::icon(&name).map(|data| (name, data)))
                    .collect()
            });
            let _ = send.send(icons);
        })
        .map_err(|error| error.to_string())?;
        receive.await.map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, names);
        Ok(HashMap::new())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSWorkspace,
    };
    use objc2_foundation::{
        NSDataBase64EncodingOptions, NSDictionary, NSPoint, NSRect, NSSize, NSString,
    };

    pub(super) fn icon(name: &str) -> Option<String> {
        // This endpoint resolves application names, never arbitrary file paths.
        if name.is_empty() || name.len() > 255 || name.contains(['/', '\\', '\0']) {
            return None;
        }
        let app_name = match name {
            "Chrome" => "Google Chrome",
            "Claude Code" => "Claude",
            "Cursor Agent" => "Cursor",
            "Code" => "Visual Studio Code",
            _ => name,
        };
        let workspace = NSWorkspace::sharedWorkspace();
        // Snapshot groups have display names, not bundle identifiers. Launch
        // Services also finds apps outside /Applications (including user apps).
        #[allow(deprecated)]
        let path = workspace.fullPathForApplication(&NSString::from_str(app_name))?;
        if !path.to_string().ends_with(".app") {
            return None;
        }
        let image = workspace.iconForFile(&path);
        // Render a fixed-size thumbnail rather than send a full icon family.
        // 64 pixels stays sharp at 2x in both the sidebar and detail heading.
        // SAFETY: null planes asks AppKit to allocate the RGBA buffer; the
        // dimensions and layout describe 64 x 64 pixels with four 8-bit samples.
        let bitmap = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(), 64, 64, 8, 4,
                true, false, NSDeviceRGBColorSpace, 0, 0,
            )
        }?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        image.drawInRect_fromRect_operation_fraction(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0)),
            NSRect::ZERO,
            NSCompositingOperation::Copy,
            1.0,
        );
        NSGraphicsContext::restoreGraphicsState_class();
        // SAFETY: an empty dictionary contains no incorrectly typed properties.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }?;
        Some(format!(
            "data:image/png;base64,{}",
            png.base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty())
        ))
    }
}
