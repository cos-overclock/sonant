use gpui::{
    App, Application, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};
use gpui_component::{
    Root,
    button::{Button, ButtonVariants as _},
    label::Label,
};

#[cfg(target_os = "macos")]
use cocoa::{
    appkit::{
        NSApplication, NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
    },
    base::nil,
};

fn main() {
    let is_helper = std::env::args().any(|arg| arg == "--gpui-helper");

    if is_helper {
        run_gpui_helper();
        return;
    }

    eprintln!("Sonant helper binary. Run with --gpui-helper.");
}

fn run_gpui_helper() {
    Application::new().run(|cx: &mut App| {
        set_plugin_helper_activation_policy();
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(640.0), px(420.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        if cx
            .open_window(options, |window, cx| {
                let view = cx.new(|_| SonantGpuiPocView);
                cx.new(|cx| Root::new(view, window, cx))
            })
            .is_err()
        {
            cx.quit();
            return;
        }

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        cx.activate(true);
        set_plugin_helper_activation_policy();
    });
}

#[cfg(target_os = "macos")]
fn set_plugin_helper_activation_policy() {
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_plugin_helper_activation_policy() {}

struct SonantGpuiPocView;

impl Render for SonantGpuiPocView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_3()
            .bg(rgb(0x1f2937))
            .text_color(rgb(0xf9fafb))
            .child("Sonant CLAP Plugin GPUI PoC")
            .child(Label::new(
                "gpui + gpui-component helper window (spawned from plugin)",
            ))
            .child(
                Button::new("gpui-component-button")
                    .primary()
                    .label("gpui-component Button")
                    .on_click(|_, _, _| {
                        eprintln!("sonant-helper: gpui-component button clicked");
                    }),
            )
    }
}
