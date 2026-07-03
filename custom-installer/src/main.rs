#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use serde::Serialize;
use std::{
    env,
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::{Emitter, Manager, Window};
use winreg::{enums::*, RegKey};

const APP_NAME: &str = "AI";
const APP_VERSION: &str = "1.0.6";
const APP_EXE_NAME: &str = "AI.exe";
const UNINSTALL_EXE_NAME: &str = "Uninstall AI.exe";
const REGISTRY_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AI";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const APP_PAYLOAD: &[u8] = include_bytes!(env!("GEMMA_PAYLOAD_EXE"));

#[derive(Serialize, Clone)]
struct InstallProgress {
    percent: u8,
    status: String,
    detail: String,
}

#[derive(Serialize)]
struct InstallerInfo {
    app_name: String,
    version: String,
    payload_name: String,
    payload_size: usize,
    install_dir: String,
}

#[derive(Serialize)]
struct InstallResult {
    install_dir: String,
    app_path: String,
}

#[tauri::command]
fn installer_info() -> Result<InstallerInfo, String> {
    Ok(InstallerInfo {
        app_name: APP_NAME.to_string(),
        version: APP_VERSION.to_string(),
        payload_name: APP_EXE_NAME.to_string(),
        payload_size: APP_PAYLOAD.len(),
        install_dir: install_dir()?.display().to_string(),
    })
}

#[tauri::command]
fn install_app(window: Window) -> Result<InstallResult, String> {
    emit_progress(&window, 6, "preparing", "creating install directory");
    let install_dir = install_dir()?;
    fs::create_dir_all(&install_dir).map_err(|err| err.to_string())?;

    let app_path = install_dir.join(APP_EXE_NAME);
    emit_progress(&window, 28, "copying", APP_EXE_NAME);
    fs::write(&app_path, APP_PAYLOAD).map_err(|err| err.to_string())?;

    emit_progress(&window, 48, "copying", UNINSTALL_EXE_NAME);
    let uninstall_path = install_dir.join(UNINSTALL_EXE_NAME);
    let current_exe = env::current_exe().map_err(|err| err.to_string())?;
    fs::copy(current_exe, &uninstall_path).map_err(|err| err.to_string())?;

    emit_progress(&window, 68, "shortcuts", "creating desktop and start menu shortcuts");
    create_shortcuts(&app_path, &install_dir)?;

    emit_progress(&window, 86, "registry", "registering uninstaller");
    register_uninstaller(&install_dir, &app_path, &uninstall_path)?;

    emit_progress(&window, 100, "installed", "ready to launch");
    let launch_path = app_path.clone();
    let close_window = window.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        let _ = launch_installed_app(&launch_path);
        let _ = close_window.close();
    });

    Ok(InstallResult {
        install_dir: install_dir.display().to_string(),
        app_path: app_path.display().to_string(),
    })
}

#[tauri::command]
fn close_installer(window: Window) -> Result<(), String> {
    launch_existing_app();
    window.close().map_err(|err| err.to_string())
}

#[tauri::command]
fn minimize_installer(window: Window) -> Result<(), String> {
    window.minimize().map_err(|err| err.to_string())
}

#[tauri::command]
fn toggle_maximize_installer(window: Window) -> Result<(), String> {
    if window.is_maximized().map_err(|err| err.to_string())? {
        window.unmaximize().map_err(|err| err.to_string())
    } else {
        window.maximize().map_err(|err| err.to_string())
    }
}

fn main() {
    if env::args().any(|arg| arg == "--uninstall") {
        let _ = uninstall_app();
        return;
    }

    tauri::Builder::default()
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
                let _ = window.set_always_on_top(true);

                let later_window = window.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(900));
                    let _ = later_window.set_always_on_top(false);
                    let _ = later_window.set_focus();
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            installer_info,
            install_app,
            close_installer,
            minimize_installer,
            toggle_maximize_installer
        ])
        .run(tauri::generate_context!())
        .expect("failed to run custom installer");
}

fn emit_progress(window: &Window, percent: u8, status: &str, detail: &str) {
    let _ = window.emit(
        "install-progress",
        InstallProgress {
            percent,
            status: status.to_string(),
            detail: detail.to_string(),
        },
    );
}

fn install_dir() -> Result<PathBuf, String> {
    let local_app_data = env_path("LOCALAPPDATA")?;
    Ok(local_app_data
        .join("Programs")
        .join("AI"))
}

fn env_path(name: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not set"))
}

fn create_shortcuts(app_path: &Path, install_dir: &Path) -> Result<(), String> {
    let desktop = env_path("USERPROFILE")?.join("Desktop");
    let start_menu = env_path("APPDATA")?
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");

    fs::create_dir_all(&start_menu).map_err(|err| err.to_string())?;

    create_shortcut(
        &desktop.join("AI.lnk"),
        app_path,
        install_dir,
    )?;
    create_shortcut(
        &start_menu.join("AI.lnk"),
        app_path,
        install_dir,
    )?;

    Ok(())
}

fn launch_installed_app(app_path: &Path) -> Result<(), String> {
    let working_dir = app_path.parent().unwrap_or_else(|| Path::new("."));
    Command::new(app_path)
        .current_dir(working_dir)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to launch app: {err}"))
}

fn launch_existing_app() {
    if let Ok(app_path) = install_dir().map(|dir| dir.join(APP_EXE_NAME)) {
        if app_path.exists() {
            let _ = launch_installed_app(&app_path);
        }
    }
}

fn create_shortcut(shortcut_path: &Path, target_path: &Path, working_dir: &Path) -> Result<(), String> {
    let script = format!(
        "$shell = New-Object -ComObject WScript.Shell\n\
         $shortcut = $shell.CreateShortcut('{}')\n\
         $shortcut.TargetPath = '{}'\n\
         $shortcut.WorkingDirectory = '{}'\n\
         $shortcut.IconLocation = '{},0'\n\
         $shortcut.Save()\n",
        ps_quote(shortcut_path),
        ps_quote(target_path),
        ps_quote(working_dir),
        ps_quote(target_path)
    );
    run_powershell(&script)
}

fn register_uninstaller(
    install_dir: &Path,
    app_path: &Path,
    uninstall_path: &Path,
) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(REGISTRY_KEY)
        .map_err(|err| err.to_string())?;

    let uninstall = format!("\"{}\" --uninstall", uninstall_path.display());
    key.set_value("DisplayName", &APP_NAME)
        .map_err(|err| err.to_string())?;
    key.set_value("DisplayVersion", &APP_VERSION)
        .map_err(|err| err.to_string())?;
    key.set_value("Publisher", &"xocat")
        .map_err(|err| err.to_string())?;
    key.set_value("InstallLocation", &install_dir.display().to_string())
        .map_err(|err| err.to_string())?;
    key.set_value("DisplayIcon", &app_path.display().to_string())
        .map_err(|err| err.to_string())?;
    key.set_value("UninstallString", &uninstall)
        .map_err(|err| err.to_string())?;
    key.set_value("QuietUninstallString", &uninstall)
        .map_err(|err| err.to_string())?;
    key.set_value("NoModify", &1u32)
        .map_err(|err| err.to_string())?;
    key.set_value("NoRepair", &1u32)
        .map_err(|err| err.to_string())?;
    key.set_value("EstimatedSize", &((APP_PAYLOAD.len() / 1024) as u32))
        .map_err(|err| err.to_string())?;

    Ok(())
}

fn uninstall_app() -> Result<(), String> {
    let install_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or(install_dir()?);

    let _ = remove_shortcuts();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let _ = hkcu.delete_subkey_all(REGISTRY_KEY);

    let cleanup = format!(
        "timeout /t 1 /nobreak > nul & rmdir /s /q \"{}\"",
        install_dir.display()
    );
    let mut command = Command::new("cmd");
    command.args(["/C", &cleanup]);
    hide_command_window(&mut command);
    let _ = command.spawn();
    Ok(())
}

fn remove_shortcuts() -> Result<(), String> {
    let paths = [
        env_path("USERPROFILE")?
            .join("Desktop")
            .join("AI.lnk"),
        env_path("APPDATA")?
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("AI.lnk"),
    ];

    for path in paths {
        let _ = fs::remove_file(path);
    }

    Ok(())
}

fn ps_quote(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

fn run_powershell(script: &str) -> Result<(), String> {
    let bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut command = Command::new("powershell");
    command.args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ]);
    hide_command_window(&mut command);
    let status = command.status()
        .map_err(|err| err.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("powershell failed with {status}"))
    }
}

fn hide_command_window(command: &mut Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
}
