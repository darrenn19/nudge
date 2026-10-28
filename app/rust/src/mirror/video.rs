//! Hardware decode and display on Windows, from `spikes/mirror-latency`
//! (measured there: decode + draw ~1 ms median).
//!
//! Media Foundation's H.264 / H.265 decoder runs on the GPU (DXVA) and hands
//! back GPU textures. The D3D11 video processor converts them to RGB (with the
//! right colour matrix and range) and scales them smoothly into the window's
//! swap chain. Nothing is copied back to the CPU.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::core::{Error, Interface, PCWSTR, Result, w};

/// A decoded picture, still on the GPU.
pub type Picture = IMFSample;

const WAITABLE: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

// One mirror window at a time (see `super::show`).
static RESIZED: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicBool = AtomicBool::new(false);

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            RESIZED.store(true, Relaxed);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wp.0 {
                0x1B => unsafe {
                    let _ = DestroyWindow(hwnd); // Esc
                },
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOSED.store(true, Relaxed);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Handles window messages. Returns false once the window is closed.
pub fn pump() -> bool {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    !CLOSED.load(Relaxed)
}

/// Closes the window (from its thread).
pub fn close(video: &Video) {
    if let Some((hwnd, _, _)) = &video.window {
        unsafe {
            let _ = DestroyWindow(*hwnd);
        }
    }
    CLOSED.store(true, Relaxed);
}

/// A window `width` x `height` (client area, physical pixels), black until
/// the first frame.
fn create_window(title: &str, width: i32, height: i32) -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("NudgeMirror");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            // The app's own icon (resource 101 in the Flutter runner).
            hIcon: LoadIconW(instance, PCWSTR(101 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut r = RECT { left: 0, top: 0, right: width, bottom: height };
        AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false)?;
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            instance,
            None,
        )?;
        let _ = SetForegroundWindow(hwnd);
        Ok(hwnd)
    }
}

/// A client area fitting a `w` x `h` picture on this screen (90% of it at most).
fn fitted(w: i32, h: i32) -> (i32, i32) {
    let screen_h = unsafe { GetSystemMetrics(SM_CYFULLSCREEN) } as f64;
    let screen_w = unsafe { GetSystemMetrics(SM_CXFULLSCREEN) } as f64;
    let scale = (screen_h * 0.9 / h as f64).min(screen_w * 0.9 / w as f64).min(1.0);
    ((w as f64 * scale) as i32, (h as f64 * scale) as i32)
}

/// What the decoder says about the current stream.
#[derive(Clone, Copy, Debug)]
struct StreamInfo {
    /// Decoded texture size (may include padding rows/columns).
    coded: (u32, u32),
    /// The visible part of it.
    visible: RECT,
    bt709: bool,
    full_range: bool,
}

struct Decoder {
    mft: IMFTransform,
    hevc: bool,
    info: Option<StreamInfo>,
}

impl Decoder {
    fn new(hevc: bool, manager: &IMFDXGIDeviceManager) -> Result<Self> {
        unsafe {
            let subtype = if hevc { MFVideoFormat_HEVC } else { MFVideoFormat_H264 };
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                None,
                &mut list,
                &mut count,
            )?;
            let activates = std::slice::from_raw_parts_mut(list, count as usize);
            let mft = activates.first().and_then(|a| a.as_ref()).map(|a| a.ActivateObject::<IMFTransform>());
            for a in activates.iter_mut() {
                drop(a.take());
            }
            CoTaskMemFree(Some(list as *const c_void));
            let mft = match mft {
                Some(m) => m?,
                None => {
                    let what = if hevc {
                        "No H.265 decoder: install \"HEVC Video Extensions\" from the Microsoft Store, or use H.264"
                    } else {
                        "No H.264 decoder found"
                    };
                    return Err(Error::new(MF_E_TOPO_CODEC_NOT_FOUND, what));
                }
            };

            let attrs = mft.GetAttributes()?;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) == 0 {
                return Err(Error::new(E_FAIL, "the decoder can't use the GPU"));
            }
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
            // Without these the decoder holds several frames back before output.
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            let _ = attrs.SetUINT32(&CODECAPI_AVLowLatencyMode, 1);

            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &t, 0)?;

            let mut d = Decoder { mft, hevc, info: None };
            let _ = d.set_output_type(); // may only be possible after the first frame
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            if d.mft.GetOutputStreamInfo(0)?.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
                return Err(Error::new(E_FAIL, "the decoder isn't handing out GPU frames"));
            }
            Ok(d)
        }
    }

    /// Picks NV12 output and reads the frame size, visible area and colour format.
    fn set_output_type(&mut self) -> Result<()> {
        unsafe {
            let mut i = 0;
            let t = loop {
                let t = self.mft.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    break t;
                }
                i += 1;
            };
            self.mft.SetOutputType(0, &t, 0)?;

            let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let coded = ((size >> 32) as u32, size as u32);
            let mut visible = RECT { left: 0, top: 0, right: coded.0 as i32, bottom: coded.1 as i32 };
            let mut area = MFVideoArea::default();
            let blob = std::slice::from_raw_parts_mut(
                &mut area as *mut MFVideoArea as *mut u8,
                std::mem::size_of::<MFVideoArea>(),
            );
            if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None).is_ok() && area.Area.cx > 0 {
                visible = RECT {
                    left: area.OffsetX.value as i32,
                    top: area.OffsetY.value as i32,
                    right: area.OffsetX.value as i32 + area.Area.cx,
                    bottom: area.OffsetY.value as i32 + area.Area.cy,
                };
            }
            // The phone asks for BT.709, limited range; trust what the stream says.
            let bt709 = t.GetUINT32(&MF_MT_YUV_MATRIX).map_or(true, |m| m != MFVideoTransferMatrix_BT601.0 as u32);
            let full_range =
                t.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).is_ok_and(|r| r == MFNominalRange_0_255.0 as u32);
            let info = StreamInfo { coded, visible, bt709, full_range };
            if self.info.is_none_or(|old| format!("{old:?}") != format!("{info:?}")) {
                log::info!(
                    "mirror stream: {} {}x{} ({}, {} range)",
                    if self.hevc { "H.265" } else { "H.264" },
                    visible.right - visible.left,
                    visible.bottom - visible.top,
                    if bt709 { "BT.709" } else { "BT.601" },
                    if full_range { "full" } else { "limited" },
                );
            }
            self.info = Some(info);
            Ok(())
        }
    }

    /// Decodes one access unit; returns the newest frame it produced, if any.
    fn decode(&mut self, data: &[u8], time_us: u64) -> Result<Option<IMFSample>> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time_us as i64 * 10)?;

            let mut newest = self.drain()?;
            match self.mft.ProcessInput(0, &sample, 0) {
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    newest = self.drain()?.or(newest);
                    self.mft.ProcessInput(0, &sample, 0)?;
                }
                r => r?,
            }
            Ok(self.drain()?.or(newest))
        }
    }

    fn drain(&mut self) -> Result<Option<IMFSample>> {
        let mut newest = None;
        loop {
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = unsafe { self.mft.ProcessOutput(0, &mut out, &mut status) };
            let sample = unsafe { ManuallyDrop::take(&mut out[0].pSample) };
            drop(unsafe { ManuallyDrop::take(&mut out[0].pEvents) });
            match r {
                Ok(()) => newest = sample.or(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.set_output_type()?,
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct Video {
    device: ID3D11Device,
    /// Kept with the device it belongs to.
    _context: ID3D11DeviceContext,
    vdevice: ID3D11VideoDevice,
    vcontext: ID3D11VideoContext1,
    manager: IMFDXGIDeviceManager,
    /// The window, its swap chain, and the swap chain's "ready for a frame" signal.
    window: Option<(HWND, IDXGISwapChain1, HANDLE)>,
    /// The picture size the window was last fitted to.
    fitted_to: Option<(i32, i32)>,
    processor: Option<((u32, u32), ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)>,
    decoder: Option<Decoder>,
}

impl Video {
    /// Opens the window (phone-shaped until the first frame says otherwise)
    /// and gets the GPU and decoder ready, so the first frame shows at once.
    pub fn new(title: &str) -> Result<Self> {
        unsafe {
            // Without this, Windows renders the window at 100% and stretches it
            // to the display's scale (e.g. 200%): a blurry picture. This
            // thread only: the app's own window is left as it is.
            let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            CLOSED.store(false, Relaxed);
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device: ID3D11Device = device.unwrap();
            let context = context.unwrap();
            // Media Foundation uses the device from its own threads.
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
            // Don't let frames queue up behind the display.
            device.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1)?;

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(&device, token)?;

            let mut v = Video {
                vdevice: device.cast()?,
                vcontext: context.cast()?,
                device,
                _context: context,
                manager,
                window: None,
                fitted_to: None,
                processor: None,
                decoder: None,
            };
            v.decoder = Some(Decoder::new(false, &v.manager)?);
            let (w, h) = fitted(1056, 2304);
            v.open_window(title, w, h)?;
            Ok(v)
        }
    }

    /// Decodes one frame; returns the newest decoded picture, if any.
    pub fn decode(&mut self, data: &[u8], hevc: bool, time_us: u64) -> Result<Option<IMFSample>> {
        if self.decoder.as_ref().is_none_or(|d| d.hevc != hevc) {
            self.decoder = None;
            self.decoder = Some(Decoder::new(hevc, &self.manager)?);
        }
        let d = self.decoder.as_mut().unwrap();
        match d.decode(data, time_us) {
            Ok(s) => Ok(s),
            Err(e) => {
                self.decoder = None; // start fresh at the next keyframe
                Err(e)
            }
        }
    }

    fn stream_info(&self) -> Option<StreamInfo> {
        self.decoder.as_ref().and_then(|d| d.info)
    }

    fn processor(&mut self, coded: (u32, u32)) -> Result<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)> {
        if let Some((size, e, p)) = &self.processor {
            if *size == coded {
                return Ok((e.clone(), p.clone()));
            }
        }
        unsafe {
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
                InputWidth: coded.0,
                InputHeight: coded.1,
                OutputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
                OutputWidth: coded.0,
                OutputHeight: coded.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
            };
            let e = self.vdevice.CreateVideoProcessorEnumerator(&desc)?;
            let p = self.vdevice.CreateVideoProcessor(&e, 0)?;
            self.processor = Some((coded, e.clone(), p.clone()));
            Ok((e, p))
        }
    }

    /// Converts and scales a decoded frame into `target`, fitted inside `area`.
    fn blit(&mut self, sample: &IMFSample, target: &ID3D11Texture2D, target_size: (u32, u32), fit: bool) -> Result<()> {
        let info = self.stream_info().ok_or_else(|| Error::new(E_FAIL, "no stream info"))?;
        let (enumerator, vp) = self.processor(info.coded)?;
        unsafe {
            let dxgi: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
            let mut texture: Option<ID3D11Texture2D> = None;
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut texture as *mut _ as *mut *mut c_void)?;
            let texture = texture.unwrap();
            let index = dxgi.GetSubresourceIndex()?;

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: index },
                },
            };
            let mut input = None;
            self.vdevice.CreateVideoProcessorInputView(&texture, &enumerator, &in_desc, Some(&mut input))?;
            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output = None;
            self.vdevice.CreateVideoProcessorOutputView(target, &enumerator, &out_desc, Some(&mut output))?;
            let output = output.unwrap();

            let src = info.visible;
            let (tw, th) = (target_size.0 as i32, target_size.1 as i32);
            let full = RECT { left: 0, top: 0, right: tw, bottom: th };
            let dest = if fit {
                // Keep the phone's shape; centre it with black bars.
                let (sw, sh) = ((src.right - src.left) as f64, (src.bottom - src.top) as f64);
                let scale = (tw as f64 / sw).min(th as f64 / sh);
                let (dw, dh) = ((sw * scale).round() as i32, (sh * scale).round() as i32);
                let (x, y) = ((tw - dw) / 2, (th - dh) / 2);
                RECT { left: x, top: y, right: x + dw, bottom: y + dh }
            } else {
                full
            };

            let vc = &self.vcontext;
            vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false); // no driver "enhancements"
            vc.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src));
            vc.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dest));
            vc.VideoProcessorSetOutputTargetRect(&vp, true, Some(&full));
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } },
            };
            vc.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
            let in_space = match (info.bt709, info.full_range) {
                (true, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                (true, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
                (false, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
                (false, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
            };
            vc.VideoProcessorSetStreamColorSpace1(&vp, 0, in_space);
            vc.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(input),
                ..Default::default()
            };
            let mut streams = [stream];
            let r = vc.VideoProcessorBlt(&vp, &output, 0, &streams);
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            r
        }
    }

    /// True if the display can take a frame now, without waiting. When it returns
    /// true, call `present` next: the check uses up the slot.
    pub fn ready_to_present(&self) -> bool {
        match &self.window {
            None => true,
            Some((_, _, ready)) => unsafe { WaitForSingleObject(*ready, 0) == WAIT_OBJECT_0 },
        }
    }

    fn open_window(&mut self, title: &str, width: i32, height: i32) -> Result<()> {
        {
            let hwnd = create_window(title, width, height)?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: WAITABLE.0 as u32,
                ..Default::default()
            };
            let (swapchain, ready) = unsafe {
                let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
                let sc = factory.CreateSwapChainForHwnd(&self.device, hwnd, &desc, None, None)?;
                // At most one frame queued; the handle is signalled when it's taken.
                let sc2: IDXGISwapChain2 = sc.cast()?;
                sc2.SetMaximumFrameLatency(1)?;
                (sc, sc2.GetFrameLatencyWaitableObject())
            };
            self.window = Some((hwnd, swapchain, ready));
            RESIZED.store(false, Relaxed);
        }
        Ok(())
    }

    /// Shows a decoded frame in the window.
    pub fn present(&mut self, sample: &IMFSample) -> Result<()> {
        let Some(info) = self.stream_info() else { return Ok(()) };
        let Some((hwnd, swapchain, _)) = self.window.clone() else { return Ok(()) };
        let picture = (info.visible.right - info.visible.left, info.visible.bottom - info.visible.top);
        if self.fitted_to != Some(picture) {
            // First frame, or the phone turned: give the window its shape.
            self.fit_window(hwnd, picture);
        }
        unsafe {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client)?;
            let size = ((client.right - client.left).max(1) as u32, (client.bottom - client.top).max(1) as u32);
            if RESIZED.swap(false, Relaxed) {
                swapchain.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, WAITABLE)?;
            }
            let back: ID3D11Texture2D = swapchain.GetBuffer(0)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut desc);
            self.blit(sample, &back, (desc.Width, desc.Height), true)?;
            drop(back);
            // Interval 0: hand the frame to the compositor now, don't wait for vsync.
            swapchain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }
}

impl Video {
    /// Resizes the window to `picture`'s shape. The first time, to the size
    /// `fitted` picks; after that the window keeps its longer side, so a phone
    /// turning sideways keeps about the same window size.
    fn fit_window(&mut self, hwnd: HWND, picture: (i32, i32)) {
        let first = self.fitted_to.is_none();
        self.fitted_to = Some(picture);
        unsafe {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client).is_err() {
                return;
            }
            let (cw, ch) = (client.right - client.left, client.bottom - client.top);
            let (pw, ph) = (picture.0 as f64, picture.1 as f64);
            let (w, h) = if first {
                fitted(picture.0, picture.1)
            } else {
                let scale = cw.max(ch) as f64 / pw.max(ph);
                ((pw * scale) as i32, (ph * scale) as i32)
            };
            if (w, h) == (cw, ch) {
                return;
            }
            let mut r = RECT { left: 0, top: 0, right: w, bottom: h };
            let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
            let _ = SetWindowPos(hwnd, None, 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER);
        }
    }
}
