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
    cpu: Option<software::Renderer>,
    fails: u32,
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
        let cpu = if gpu.is_none() { Some(software::Renderer::new(window, Default::default())?) } else { None };
        baseview::nylog(if gpu.is_some() { "renderer: Vulkan" } else { "renderer: CPU" });
        Ok(Self { gpu, cpu, fails: 0 })
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
            // keep a copy of the texture updates so a late fallback to the CPU renderer can start from scratch
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                g.render(bg_color, physical_size, pixels_per_point, egui_ctx, full_output)
            }))
            .unwrap_or(false);
            if ok {
                self.fails = 0;
            } else {
                self.fails += 1;
                if self.fails >= 40 {
                    baseview::nylog("Vulkan keeps failing, switching to the CPU renderer");
                    self.gpu = None;
                    if let Ok(c) = software::Renderer::new(window, Default::default()) {
                        self.cpu = Some(c);
                    }
                    // make egui resend every texture
                    egui_ctx.set_fonts(egui::FontDefinitions::default());
                }
            }
            return;
        }
        if let Some(c) = self.cpu.as_mut() {
            c.render(window, bg_color, physical_size, pixels_per_point, egui_ctx, full_output);
        }
    }
}
