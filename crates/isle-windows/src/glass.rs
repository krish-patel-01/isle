// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The glass look's backdrop: what lies under the island, as GNOME Shell's glass copies it (glass.js). Windows has
//! no blur an always-on-top, never-active window can cut to the island's shape, so Isle looks for itself: the
//! island's display is captured (Windows.Graphics.Capture, the frames the GPU already composes, only as something
//! on it changes), the island's own window left out of that capture, and the part behind the window cut out and
//! scaled down to a quarter on the GPU. The page blurs and tones it as GNOME's shader does (ui/js/glass.js).
//!
//! Light: no frames while nothing on the display changes, at most `FRAME` apart while it does, and none at all
//! while glass is off or the island is hidden. Left out of capture, the island is missing from screenshots and
//! screen sharing too while glass is on (Settings says so).

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};
use windows::core::{Interface, factory};

/// Least time between two frames sent to the page while what is behind keeps changing (a video, a page
/// scrolling): about 30 a second, as GNOME's glass blurs a resting island's backdrop.
const FRAME: Duration = Duration::from_millis(33);
/// Scaled down by 2^LEVEL on each side (a mip level): the page blurs it anyway.
const LEVEL: u32 = 2;

/// The latest picture of what is behind the island's window: `width` × `height` BGRA, top row first.
#[derive(Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Capture as the page asks for it: on while glass shows, off otherwise.
#[derive(Default)]
pub struct Glass {
    stop: Mutex<Option<Sender<()>>>,
    pub frame: Arc<Mutex<Frame>>,
}

impl Glass {
    /// Starts capturing what is behind the island's window, or stops; the window is left out of every capture
    /// while it runs.
    pub fn set(&self, handle: &tauri::AppHandle, on: bool) {
        let mut stop = self.stop.lock().unwrap();
        if on == stop.is_some() {
            return;
        }
        let Some(window) = handle.get_webview_window("island") else {
            return;
        };
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let hwnd = HWND(hwnd.0);
        // SAFETY: the island's own window, a valid handle while Isle runs.
        unsafe {
            let _ = SetWindowDisplayAffinity(hwnd, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
        }
        if !on {
            // The capture thread ends as its sender goes.
            *stop = None;
            return;
        }
        let (tx, rx) = channel();
        *stop = Some(tx);
        let frame = self.frame.clone();
        let handle = handle.clone();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("glass".into())
            .spawn(move || {
                if let Err(err) = run(HWND(hwnd as *mut _), rx, &frame, &handle) {
                    log::warn!(target: "isle::glass", "no backdrop for the glass: {err:#}");
                    let _ = handle.emit_to("island", "glass-unavailable", ());
                }
            })
            .ok();
    }
}

/// Captures until `stop` says so (or goes), sending each new picture to the page.
fn run(hwnd: HWND, stop: Receiver<()>, frame: &Mutex<Frame>, handle: &tauri::AppHandle) -> anyhow::Result<()> {
    let capture = Capture::new(hwnd)?;
    let (tx, rx) = channel::<()>();
    capture.pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
        let _ = tx.send(());
        Ok(())
    }))?;
    capture.session.StartCapture()?;
    let mut last = Instant::now() - FRAME;
    let mut scratch = Scratch::default();
    loop {
        // Woken by a new frame; checking now and then whether to stop.
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !matches!(stop.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
            break;
        }
        // At most one picture per FRAME: wait out the rest, then take the newest frame the pool holds.
        let since = last.elapsed();
        if since < FRAME {
            std::thread::sleep(FRAME - since);
        }
        while rx.try_recv().is_ok() {}
        let Some(picture) = capture.latest(hwnd, &mut scratch)? else {
            continue;
        };
        last = Instant::now();
        let mut current = frame.lock().unwrap();
        if current.width == picture.width && current.height == picture.height && current.pixels == picture.pixels {
            continue;
        }
        *current = picture;
        drop(current);
        let _ = handle.emit_to("island", "glass-frame", ());
    }
    let _ = capture.session.Close();
    let _ = capture.pool.Close();
    Ok(())
}

/// The textures one size of crop needs, kept while it stays that size.
#[derive(Default)]
struct Scratch {
    size: (u32, u32),
    mips: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, ID3D11Texture2D)>,
}

struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    rt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// The display's size as captured, and its top left on the desktop.
    size: Mutex<(i32, i32)>,
    origin: (i32, i32),
}

impl Capture {
    fn new(hwnd: HWND) -> anyhow::Result<Self> {
        let mut device = None;
        let mut context = None;
        // SAFETY: out parameters for a new hardware device and its context.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device: ID3D11Device = device.ok_or_else(|| anyhow::anyhow!("no Direct3D device"))?;
        let context = context.ok_or_else(|| anyhow::anyhow!("no Direct3D context"))?;
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: the device just made.
        let rt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;

        // SAFETY: the island's window; its display, or the main one.
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        // SAFETY: a display handle and its info's size set.
        unsafe { GetMonitorInfoW(monitor, &mut info) }.ok()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: a valid display handle.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &rt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        // Without the yellow frame Windows draws round a captured display, where it allows that.
        let _ = session.SetIsBorderRequired(false);
        Ok(Self {
            device,
            context,
            rt_device,
            pool,
            session,
            size: Mutex::new((size.Width, size.Height)),
            origin: (info.rcMonitor.left, info.rcMonitor.top),
        })
    }

    /// The newest frame's part behind the island's window, scaled down; None when there is no new frame.
    fn latest(&self, hwnd: HWND, scratch: &mut Scratch) -> anyhow::Result<Option<Frame>> {
        // The newest of what the pool holds; the older ones go back to it.
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        let Some(frame) = newest else {
            return Ok(None);
        };
        // The display changed size (resolution, scale): the pool follows, and the next frame is used.
        let content = frame.ContentSize()?;
        {
            let mut size = self.size.lock().unwrap();
            if (content.Width, content.Height) != *size {
                *size = (content.Width, content.Height);
                self.pool.Recreate(&self.rt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content)?;
                return Ok(None);
            }
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the frame's own texture, alive as long as `frame`.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

        // Where the window is, in the captured display's pixels.
        let mut rect = RECT::default();
        // SAFETY: the island's window and a rectangle to fill.
        unsafe { GetWindowRect(hwnd, &mut rect)? };
        let left = (rect.left - self.origin.0).clamp(0, content.Width) as u32;
        let top = (rect.top - self.origin.1).clamp(0, content.Height) as u32;
        let right = (rect.right - self.origin.0).clamp(0, content.Width) as u32;
        let bottom = (rect.bottom - self.origin.1).clamp(0, content.Height) as u32;
        let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
        if width < 4 || height < 4 {
            return Ok(None);
        }
        if scratch.size != (width, height) || scratch.mips.is_none() {
            scratch.mips = Some(self.textures(width, height)?);
            scratch.size = (width, height);
        }
        let (mips, view, staging) = scratch.mips.as_ref().unwrap();
        let (small_w, small_h) = ((width >> LEVEL).max(1), (height >> LEVEL).max(1));
        let crop = D3D11_BOX { left, top, front: 0, right, bottom, back: 1 };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: textures made on this device, the box inside the frame, the staging texture mapped and unmapped
        // here, its rows read within `RowPitch` × height.
        let pixels = unsafe {
            self.context.CopySubresourceRegion(mips, 0, 0, 0, 0, &texture, 0, Some(&crop));
            self.context.GenerateMips(view);
            self.context.CopySubresourceRegion(staging, 0, 0, 0, 0, mips, LEVEL, None);
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut pixels = Vec::with_capacity((small_w * small_h * 4) as usize);
            for row in 0..small_h {
                let start = mapped.pData.cast::<u8>().add((row * mapped.RowPitch) as usize);
                pixels.extend_from_slice(std::slice::from_raw_parts(start, (small_w * 4) as usize));
            }
            self.context.Unmap(staging, 0);
            pixels
        };
        Ok(Some(Frame { width: small_w, height: small_h, pixels }))
    }

    /// A texture `width` × `height` with its mip levels down to LEVEL, a view to make them, and a staging
    /// texture for LEVEL to be read on the CPU.
    fn textures(&self, width: u32, height: u32) -> anyhow::Result<(ID3D11Texture2D, ID3D11ShaderResourceView, ID3D11Texture2D)> {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: LEVEL + 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
        };
        let mut mips = None;
        let mut view = None;
        let mut staging = None;
        // SAFETY: descriptions filled in full; out parameters for the new textures and view.
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut mips))?;
            let mips = mips.as_ref().unwrap();
            self.device.CreateShaderResourceView(mips, None, Some(&mut view))?;
            desc.Width = (width >> LEVEL).max(1);
            desc.Height = (height >> LEVEL).max(1);
            desc.MipLevels = 1;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        }
        Ok((mips.unwrap(), view.unwrap(), staging.unwrap()))
    }
}
