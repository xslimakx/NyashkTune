#[cfg(all(feature = "opengl", not(target_os = "linux")))]
mod opengl;
#[cfg(all(feature = "opengl", not(target_os = "linux")))]
pub use opengl::renderer::{GraphicsConfig, Renderer};

// NyashkTune: on Linux the GUI is drawn through Vulkan (wgpu) and falls back to the CPU renderer
// (software.rs) when Vulkan is missing or misbehaves. Set NYASHK_RENDER=cpu to force the CPU path.
#[cfg(target_os = "linux")]
mod software;
#[cfg(target_os = "linux")]
mod vulkan;
#[cfg(target_os = "linux")]
pub use vulkan::GraphicsConfig;

#[cfg(target_os = "linux")]
pub struct Renderer {
    gpu: Option<vulkan::Renderer>,
    /// Always present: draws when there is no GPU, and while the GPU draws it keeps a copy of
    /// every texture, so a later fallback starts with all pictures (egui sends each only once).
    cpu: Option<software::Renderer>,
    /// consecutive skipped frames (minimised / hidden window, surface timeout) and crashes
    skipped: u32,
    crashes: u32,
}

#[cfg(target_os = "linux")]
impl Renderer {
    pub fn new(window: &baseview::Window, config: GraphicsConfig) -> Result<Self, software::SoftwareError> {
        let want = std::env::var("NYASHK_RENDER").unwrap_or_default().to_lowercase();
        let mut gpu = None;
        if want != "cpu" {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| vulkan::Renderer::new(window, config.clone())));
            match r {
                Ok(Ok(g)) => gpu = Some(g),
                Ok(Err(e)) => baseview::nylog(&format!("Vulkan unavailable ({e}), using the CPU renderer")),
                Err(_) => baseview::nylog("Vulkan init panicked, using the CPU renderer"),
            }
        }
        let cpu = Some(software::Renderer::new(window, Default::default())?);
        baseview::nylog(if gpu.is_some() { "renderer: Vulkan" } else { "renderer: CPU" });
        Ok(Self { gpu, cpu, skipped: 0, crashes: 0 })
    }

    /// The window contents were lost (exposed / remapped): present a full frame next time.
    pub fn force_redraw(&mut self) {
        if let Some(c) = self.cpu.as_mut() {
            c.force_redraw();
        }
    }

    pub fn max_texture_side(&self) -> usize {
        match &self.gpu {
            Some(g) => g.max_texture_side().min(8192),
            None => 8192,
        }
    }

    pub fn render(
        &mut self,
        window: &baseview::Window,
        bg_color: egui::Rgba,
        physical_size: baseview::PhySize,
        pixels_per_point: f32,
        egui_ctx: &mut egui::Context,
        full_output: &mut egui::FullOutput,
    ) {
        if let Some(g) = self.gpu.as_mut() {
            if let Some(c) = self.cpu.as_mut() {
                c.apply_textures(&full_output.textures_delta.set);
                c.free_textures(&full_output.textures_delta.free);
            }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                g.render(bg_color, physical_size, pixels_per_point, egui_ctx, full_output)
            }));
            match r {
                Ok(true) => {
                    self.skipped = 0;
                    self.crashes = 0;
                }
                // a skipped frame is normal while the window is minimised or hidden (alt-tab):
                // only a very long run of them means the surface is really broken
                Ok(false) if physical_size.width == 0 || physical_size.height == 0 => {}
                Ok(false) => self.skipped += 1,
                Err(_) => self.crashes += 1,
            }
            if self.crashes >= 3 || self.skipped >= 1800 {
                baseview::nylog(&format!("Vulkan failing (crashes {}, skipped frames {}), switching to the CPU renderer", self.crashes, self.skipped));
                self.gpu = None;
                if let Some(c) = self.cpu.as_mut() {
                    c.force_redraw();
                }
            }
            return;
        }
        if let Some(c) = self.cpu.as_mut() {
            c.render(window, bg_color, physical_size, pixels_per_point, egui_ctx, full_output);
        }
    }
}
