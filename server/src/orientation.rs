//! Display orientation management using Windows GDI ChangeDisplaySettingsEx.

#![cfg(windows)]

use windows::Win32::Graphics::Gdi::*;

/// Apply target orientation (0, 90, 180, 270) to the monitor named `monitor_name`.
/// Returns the resulting (width, height) for the display.
pub fn apply_orientation(
    monitor_name: &str,
    target_orientation: u16,
    curr_w: u32,
    curr_h: u32,
) -> (u32, u32) {
    let target_dmdo = match target_orientation {
        0 => DMDO_DEFAULT,
        90 => DMDO_90,
        180 => DMDO_180,
        270 => DMDO_270,
        _ => {
            eprintln!("invalid orientation {target_orientation}; defaulting to 0");
            DMDO_DEFAULT
        }
    };

    let name_wide: Vec<u16> = format!("{}\0", monitor_name).encode_utf16().collect();
    let pcwstr = windows::core::PCWSTR(name_wide.as_ptr());

    let mut dm = DEVMODEW::default();
    dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;

    let (mut target_w, mut target_h) = if target_orientation == 90 || target_orientation == 270 {
        if curr_w > curr_h { (curr_h, curr_w) } else { (curr_w, curr_h) }
    } else {
        if curr_w < curr_h { (curr_h, curr_w) } else { (curr_w, curr_h) }
    };

    let ok = unsafe { EnumDisplaySettingsExW(pcwstr, ENUM_CURRENT_SETTINGS, &mut dm, EDS_ROTATEDMODE) };
    let (w_before, h_before) = if ok.as_bool() {
        (dm.dmPelsWidth, dm.dmPelsHeight)
    } else {
        (curr_w, curr_h)
    };

    if ok.as_bool() {
        let curr_dmdo = unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation };
        let curr_is_portrait = curr_dmdo == DMDO_90 || curr_dmdo == DMDO_270;
        let target_is_portrait = target_dmdo == DMDO_90 || target_dmdo == DMDO_270;

        if curr_is_portrait != target_is_portrait {
            std::mem::swap(&mut dm.dmPelsWidth, &mut dm.dmPelsHeight);
        }
        dm.Anonymous1.Anonymous2.dmDisplayOrientation = target_dmdo;
        dm.dmFields |= DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION | DM_POSITION;

        let res = unsafe {
            ChangeDisplaySettingsExW(
                pcwstr,
                Some(&dm),
                None,
                CDS_UPDATEREGISTRY | CDS_NORESET,
                None,
            )
        };
        let commit = unsafe {
            ChangeDisplaySettingsExW(
                windows::core::PCWSTR::null(),
                None,
                None,
                CDS_TYPE(0),
                None,
            )
        };
        println!(
            "ChangeDisplaySettingsEx for {} (orientation {}°): res={:?}, commit={:?}",
            monitor_name, target_orientation, res, commit
        );

        // Re-read current settings
        let mut dm_after = DEVMODEW::default();
        dm_after.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if unsafe { EnumDisplaySettingsW(pcwstr, ENUM_CURRENT_SETTINGS, &mut dm_after) }.as_bool() {
            println!(
                "monitor size before: {}x{}, after: {}x{}",
                w_before, h_before, dm_after.dmPelsWidth, dm_after.dmPelsHeight
            );
            if dm_after.dmPelsWidth > 0 && dm_after.dmPelsHeight > 0 {
                target_w = dm_after.dmPelsWidth;
                target_h = dm_after.dmPelsHeight;
            }
        } else {
            println!(
                "monitor size before: {}x{}, after: {}x{}",
                w_before, h_before, target_w, target_h
            );
        }
    } else {
        println!(
            "monitor size before: {}x{}, after: {}x{}",
            w_before, h_before, target_w, target_h
        );
    }

    (target_w, target_h)
}
