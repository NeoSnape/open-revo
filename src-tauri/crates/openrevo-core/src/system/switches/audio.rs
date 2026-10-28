use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// Read real-time microphone active status (true = active/unmuted, false = muted)
pub fn get_microphone_state() -> Option<bool> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let muted = endpoint_volume.GetMute().ok()?;
        Some(!muted.as_bool())
    }
}

/// Set microphone mute state (enable: true = unmute/active, false = mute)
pub fn set_microphone_state(enable: bool) -> Result<bool, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("Failed CoCreateInstance: {:?}", e))?;
        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|e| format!("Failed GetDefaultAudioEndpoint: {:?}", e))?;
        let endpoint_volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Failed Activate endpoint: {:?}", e))?;
        let muted = !enable;
        endpoint_volume.SetMute(windows::Win32::Foundation::BOOL::from(muted), std::ptr::null())
            .map_err(|e| format!("Failed SetMute: {:?}", e))?;
        Ok(enable)
    }
}
