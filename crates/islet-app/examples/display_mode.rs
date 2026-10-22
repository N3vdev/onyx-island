//! Temporary display-mode helper for frame-pacing measurements.
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_TEST, CDS_TYPE, DEVMODEW,
    DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS,
    ENUM_DISPLAY_SETTINGS_MODE,
};

fn main() {
    unsafe {
        let mut current = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        assert!(
            EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut current).as_bool()
        );
        println!(
            "current: {}x{} {}Hz {}bpp",
            current.dmPelsWidth,
            current.dmPelsHeight,
            current.dmDisplayFrequency,
            current.dmBitsPerPel
        );

        let mut frequencies = Vec::new();
        let mut index = 0;
        loop {
            let mut mode = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if !EnumDisplaySettingsW(PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(index), &mut mode)
                .as_bool()
            {
                break;
            }
            if mode.dmPelsWidth == current.dmPelsWidth
                && mode.dmPelsHeight == current.dmPelsHeight
                && mode.dmBitsPerPel == current.dmBitsPerPel
            {
                frequencies.push(mode.dmDisplayFrequency);
            }
            index += 1;
        }
        frequencies.sort_unstable();
        frequencies.dedup();
        println!("frequencies: {frequencies:?}");

        let Some(target) = std::env::args()
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return;
        };
        let mut mode = current;
        mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        mode.dmDisplayFrequency = target;
        let test = ChangeDisplaySettingsExW(PCWSTR::null(), Some(&mode), None, CDS_TEST, None);
        println!("test {target}Hz: {:?}", test);
        let result = ChangeDisplaySettingsExW(PCWSTR::null(), Some(&mode), None, CDS_TYPE(0), None);
        println!("set {target}Hz: {:?}", result);
    }
}
