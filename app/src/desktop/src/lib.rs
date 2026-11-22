pub mod actions;
pub mod backup;
mod commands;
pub mod files;
pub mod git;
pub mod github;
pub mod history;
pub mod ipc;
pub mod proj;
pub mod projects;
#[cfg(test)]
mod testing;
pub mod theme;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        // The file watcher logs "unable to remove watch descriptor" (harmless: the
                        // kernel already dropped it when a watched folder was deleted) once per
                        // deleted folder, which floods the log during builds.
                        .level_for("notify", log::LevelFilter::Warn)
                        .build(),
                )?;
            }
            app.manage(commands::AppState::new(app.handle())?);
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::git_check,
            commands::tasks_running,
            commands::task_cancel,
            commands::projects_list,
            commands::project_summary,
            commands::project_state,
            commands::project_add,
            commands::project_create,
            commands::project_clone,
            commands::project_remove,
            commands::project_reorder,
            commands::projects_watch,
            commands::session_load,
            commands::session_save,
            commands::settings_load,
            commands::settings_save,
            commands::themes_list,
            commands::themes_folder,
            commands::github_status,
            commands::github_sign_in,
            commands::github_use_gh,
            commands::github_sign_out,
            commands::graph_load,
            commands::graph_rows,
            commands::graph_find,
            commands::log_search,
            commands::log_search_cancel,
            commands::commit_detail,
            commands::file_diff,
            commands::file_blame,
            commands::work_diff,
            commands::work_stats,
            commands::action_preview,
            commands::stash_list,
            commands::action_run,
            commands::conflict_info,
            commands::open_folder,
            commands::files_list,
            commands::file_read,
            commands::file_write,
            commands::file_create,
            commands::backups_list,
            commands::backup_preview,
            commands::backup_restore,
            commands::op_log,
            commands::op_log_clear,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
