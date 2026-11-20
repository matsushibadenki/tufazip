#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod archive;
mod predictive;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    mode: String,
    format: String,
    level: u8,
    split_mb: u32,
    password: String,
    confirm: String,
    clean: bool,
    verify: bool,
    separate: bool,
    language: String,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: "compress".into(),
            format: "zip".into(),
            level: 2,
            split_mb: 0,
            password: String::new(),
            confirm: String::new(),
            clean: true,
            verify: false,
            separate: false,
            language: "auto".into(),
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    options: Options,
    inputs: Vec<PathBuf>,
    #[serde(skip)]
    previous_compression_inputs: Option<Vec<PathBuf>>,
    floating: bool,
    busy: bool,
    current: String,
    position: usize,
    total: usize,
    output: Option<PathBuf>,
    error: Option<String>,
    revision: u64,
}

#[derive(Default)]
struct Workspace {
    document: Mutex<Document>,
    cancel: AtomicBool,
}

fn update(app: &AppHandle, change: impl FnOnce(&mut Document)) {
    let state = app.state::<Workspace>();
    let mut doc = state.document.lock().unwrap();
    change(&mut doc);
    doc.revision += 1;
    let _ = app.emit("document", &*doc);
}

#[tauri::command]
fn snapshot(app: AppHandle) -> Document {
    app.state::<Workspace>().document.lock().unwrap().clone()
}

#[tauri::command]
fn configure(app: AppHandle, key: String, value: Value) -> Result<(), String> {
    let state = app.state::<Workspace>();
    let mut doc = state.document.lock().unwrap();
    if doc.busy {
        return Err("busy".into());
    }
    let mut options = serde_json::to_value(&doc.options).unwrap();
    *options.get_mut(&key).ok_or("settings")? = value;
    let options: Options = serde_json::from_value(options).map_err(|_| "settings")?;
    if !matches!(options.mode.as_str(), "compress" | "extract")
        || !matches!(options.format.as_str(), "zip" | "local")
        || options.level > 3
        || !matches!(options.language.as_str(), "auto" | "ja" | "en" | "zh")
    {
        return Err("settings".into());
    }
    if doc.options.mode != options.mode {
        doc.change_mode(&options.mode);
    } else if options.mode == "compress"
        && options.format == "local"
        && (!options.password.is_empty() || !options.confirm.is_empty())
    {
        return Err("zipPasswordOnly".into());
    } else {
        doc.options = options;
    }
    doc.output = None;
    doc.error = None;
    doc.revision += 1;
    app.emit("document", &*doc).map_err(|e| e.to_string())
}

impl Document {
    fn change_mode(&mut self, mode: &str) {
        self.options.mode = mode.into();
        self.options.password.clear();
        self.options.confirm.clear();
        if mode == "compress"
            && let Some(previous) = self.previous_compression_inputs.take()
        {
            // With no previous batch, keep the archive so the user can explicitly compress it.
            if !previous.is_empty() {
                self.inputs = previous;
            }
        } else {
            self.inputs.clear();
        }
    }

    fn add_inputs(&mut self, paths: Vec<PathBuf>) {
        if self.busy || paths.is_empty() {
            return;
        }
        if self.options.mode == "compress"
            && let [path] = paths.as_slice()
        {
            let archive_extension = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    ["zip", "tzlocal", "001"]
                        .iter()
                        .any(|known| ext.eq_ignore_ascii_case(known))
                });
            if archive_extension && path.symlink_metadata().is_ok_and(|meta| meta.is_file()) {
                self.previous_compression_inputs = Some(std::mem::take(&mut self.inputs));
                self.change_mode("extract");
            }
        }
        if self.options.mode == "extract" {
            self.inputs.clear();
        }
        for path in paths {
            if !self.inputs.contains(&path) {
                self.inputs.push(path);
            }
        }
        self.error = None;
        self.output = None;
    }
}

#[tauri::command]
async fn choose_inputs(app: AppHandle, folders: bool) {
    let extract = snapshot(app.clone()).options.mode == "extract";
    let dialog = rfd::AsyncFileDialog::new();
    let files = if extract {
        dialog.pick_file().await.map(|f| vec![f])
    } else if folders {
        dialog.pick_folders().await
    } else {
        dialog.pick_files().await
    };
    if let Some(files) = files {
        let paths = files.into_iter().map(|f| f.path().to_owned()).collect();
        update(&app, |doc| doc.add_inputs(paths));
    }
}

#[tauri::command]
fn remove_input(app: AppHandle, path: PathBuf) {
    update(&app, |doc| {
        if !doc.busy {
            doc.inputs.retain(|p| *p != path);
            doc.error = None;
            doc.output = None;
        }
    });
}

#[tauri::command]
async fn float_settings(app: AppHandle, floating: bool) -> Result<(), String> {
    if floating {
        if let Some(win) = app.get_webview_window("settings") {
            return win.set_focus().map_err(|e| e.to_string());
        }
        let win = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("index.html".into()))
            .title("TufaZIP")
            .inner_size(359., 390.)
            .min_inner_size(330., 310.)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .always_on_top(true)
            .build()
            .map_err(|e| e.to_string())?;
        let handle = app.clone();
        win.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                update(&handle, |d| d.floating = false);
            }
        });
        update(&app, |d| d.floating = true);
    } else if let Some(win) = app.get_webview_window("settings") {
        win.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn cancel(app: AppHandle) {
    app.state::<Workspace>()
        .cancel
        .store(true, Ordering::Relaxed);
}

#[tauri::command]
fn reveal(app: AppHandle) -> Result<(), String> {
    if let Some(p) = snapshot(app).output {
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn start_job(app: AppHandle) -> Result<(), String> {
    let (inputs, options) = {
        let state = app.state::<Workspace>();
        let mut doc = state.document.lock().unwrap();
        if doc.busy {
            return Err("busy".into());
        }
        if doc.inputs.is_empty() {
            return Err("empty".into());
        }
        if doc.options.mode == "compress" && doc.options.password != doc.options.confirm {
            return Err("passwordMismatch".into());
        }
        doc.busy = true;
        doc.error = None;
        doc.output = None;
        doc.current.clear();
        doc.position = 0;
        doc.total = 0;
        state.cancel.store(false, Ordering::Relaxed);
        doc.revision += 1;
        let _ = app.emit("document", &*doc);
        (doc.inputs.clone(), doc.options.clone())
    };
    let Some(dest) = rfd::AsyncFileDialog::new().pick_folder().await else {
        update(&app, |d| d.busy = false);
        return Ok(());
    };
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let last = std::cell::Cell::new(None::<Instant>);
        archive::run(&inputs, dest.path(), &options, &|name, position, total| {
            anyhow::ensure!(
                !handle.state::<Workspace>().cancel.load(Ordering::Relaxed),
                "cancelled"
            );
            if last
                .get()
                .is_none_or(|t| t.elapsed() > Duration::from_millis(125))
            {
                last.set(Some(Instant::now()));
                update(&handle, |d| {
                    d.current = name.to_owned();
                    d.position = position;
                    d.total = total;
                });
            }
            Ok(())
        })
    })
    .await;
    update(&app, |d| {
        d.busy = false;
        d.current.clear();
        match result {
            Ok(Ok(path)) => d.output = Some(path),
            Ok(Err(err)) => d.error = Some(format!("{err:#}")),
            Err(err) => d.error = Some(err.to_string()),
        }
    });
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Workspace::default())
        .on_window_event(|win, event| {
            if win.label() == "main" {
                if let WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                    update(win.app_handle(), |doc| doc.add_inputs(paths.clone()));
                }
                if matches!(event, WindowEvent::Destroyed)
                    && let Some(panel) = win.app_handle().get_webview_window("settings")
                {
                    let _ = panel.close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            configure,
            choose_inputs,
            remove_input,
            float_settings,
            cancel,
            reveal,
            start_job
        ])
        .run(tauri::generate_context!())
        .expect("Failed to launch TufaZIP");
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use std::fs;

    #[test]
    fn archive_selection_enters_extract_and_restores_compression_batch() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let text = root.path().join("notes.txt");
        fs::write(&text, [])?;
        for name in [
            "archive.zip",
            "日本語.TzLoCaL",
            "archive.ZIP",
            "archive.zip.001",
        ] {
            let archive = root.path().join(name);
            fs::write(&archive, [])?;
            let mut doc = Document::default();
            doc.add_inputs(vec![text.clone()]);
            doc.add_inputs(vec![archive.clone()]);
            assert_eq!(doc.options.mode, "extract");
            assert_eq!(doc.inputs, [archive]);
            doc.change_mode("compress");
            assert_eq!(doc.inputs.as_slice(), std::slice::from_ref(&text));
        }
        Ok(())
    }

    #[test]
    fn folder_and_mixed_selection_stay_in_compress_mode() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("folder.zip");
        fs::create_dir(&folder)?;
        let archive = root.path().join("archive.tzlocal");
        fs::write(&archive, [])?;
        let mut doc = Document::default();
        doc.add_inputs(vec![folder.clone()]);
        assert_eq!(doc.options.mode, "compress");
        doc.add_inputs(vec![folder.clone(), archive.clone()]);
        assert_eq!(doc.options.mode, "compress");
        assert_eq!(doc.inputs, [folder, archive.clone()]);

        let mut single = Document::default();
        single.add_inputs(vec![archive.clone()]);
        assert_eq!(single.options.mode, "extract");
        single.change_mode("compress");
        assert_eq!(single.inputs, [archive]);
        Ok(())
    }
}
