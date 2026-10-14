//! Desktop notifications through `notify-send` (Omarchy's mako shows them). If it isn't
//! installed, nothing happens.

pub fn send(title: &str, body: &str, icon: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=tern", &format!("--icon={icon}"), title, body])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
