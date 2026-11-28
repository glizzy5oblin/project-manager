pub mod actions;
pub mod app_data;
pub mod backup;
mod commands;
pub mod diagnostics;
pub mod files;
pub mod git;
pub mod github;
pub mod history;
pub mod host_env;
pub mod ipc;
pub mod proj;
pub mod projects;
pub mod release;
pub mod staging;
#[cfg(test)]
mod testing;
pub mod theme;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .setup(|app| {
            // The log file lives in the app data folder in every build (debug adds stdout); the
            // diagnostics command reads its tail back for the UI.
            let data_dir = app.path().app_data_dir()?;
            let logs_dir = data_dir.join("logs");
            let mut targets = vec![tauri_plugin_log::Target::new(
                tauri_plugin_log::TargetKind::Folder {
                    path: logs_dir,
                    file_name: Some("project-manager".to_string()),
                },
            )];
            if cfg!(debug_assertions) {
                targets.push(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ));
            }
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    // The file watcher logs "unable to remove watch descriptor" (harmless: the
                    // kernel already dropped it when a watched folder was deleted) once per
                    // deleted folder, which floods the log during builds.
                    .level_for("notify", log::LevelFilter::Warn)
                    .targets(targets)
                    .build(),
            )?;
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
            commands::theme_save_user,
            commands::theme_save_as,
            commands::theme_export,
            commands::theme_import,
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
            commands::rebase_range,
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
            commands::backups_import_bundle,
            commands::backups_delete,
            commands::backups_cleanup_preview,
            commands::backups_cleanup,
            commands::op_log,
            commands::op_log_clear,
            commands::staging_preview,
            commands::staging_sync,
            commands::staging_verify,
            commands::staging_records,
            commands::staging_set_override,
            commands::release_preview,
            commands::release_apply,
            commands::release_tag_preview,
            commands::release_tag_create,
            commands::release_zip,
            commands::release_changelog,
            commands::release_history_list,
            commands::release_history_record,
            commands::release_checklist,
            commands::release_github_list,
            commands::release_github_create,
            commands::release_github_update,
            commands::release_github_delete,
            commands::release_github_upload,
            commands::release_github_replace_asset,
            commands::staging_diff,
            commands::get_diagnostics,
            commands::app_data_export,
            commands::app_data_restore,
            commands::diagnostics_save,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
