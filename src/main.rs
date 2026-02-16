mod ui;

fn main() {
    let is_helper = std::env::args().any(|arg| arg == "--gpui-helper");

    if is_helper {
        ui::run_gpui_helper();
        return;
    }

    eprintln!("Sonant helper binary. Run with --gpui-helper.");
}
