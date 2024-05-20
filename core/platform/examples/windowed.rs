use rgine_logger::info;
use rgine_modules::{
    events::{EventQueue, Listener},
    standards::events::{OnShutdown, OnStart},
    Engine, Module,
};
use rgine_platform::window::{
    module::WindowRenderReadyEvent, OnWindowPlatformUpdate, WindowPlatformConfig,
    WindowPlatformEngineExt,
};

fn main() {
    let mut engine = Engine::new();
    engine.dependency::<ExampleModule>().unwrap();
    engine
        .run_windowed(WindowPlatformConfig::default())
        .unwrap();
}

struct ExampleModule;
impl Module for ExampleModule {
    type ListeningTo = (
        OnStart,
        OnWindowPlatformUpdate,
        WindowRenderReadyEvent,
        OnShutdown,
    );
    fn new(_: &mut Engine) -> rgine_modules::AnyResult<Self> {
        Ok(ExampleModule)
    }
}

impl Listener<OnStart> for ExampleModule {
    fn on_event(&mut self, _: &mut OnStart, _: &mut EventQueue) {
        info!("On start!")
    }
}
impl Listener<OnWindowPlatformUpdate> for ExampleModule {
    fn on_event(&mut self, _: &mut OnWindowPlatformUpdate, _: &mut EventQueue) {
        info!("On update!")
    }
}
impl Listener<WindowRenderReadyEvent> for ExampleModule {
    fn on_event(&mut self, _: &mut WindowRenderReadyEvent, _: &mut EventQueue) {
        info!("On render rady!")
    }
}
impl Listener<OnShutdown> for ExampleModule {
    fn on_event(&mut self, _: &mut OnShutdown, _: &mut EventQueue) {
        info!("On shutdown!")
    }
}
