//! Custom usage follows server settings and never leaves reset requests pending.

use super::session_lifecycle_requests::HistoryCapabilities;
use super::session_lifecycle_requests::recorded_params;
use super::session_lifecycle_requests::start_recording_app_server_with_history;
use super::*;
use crate::app_event::RateLimitRefreshOrigin;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn custom_provider_disabled_resets_complete_pending_picker_without_reading_usage()
-> Result<()> {
    let (mut app, mut rx, _ops) = make_test_app_with_channels().await;
    app.config.model_provider.supports_usage = true;
    app.chat_widget.sync_provider_usage(&app.config);
    set_chatgpt_auth(&mut app.chat_widget);
    let mut server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    app.chat_widget.insert_str("/usage");
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    insta::assert_snapshot!(
        "custom_provider_usage_resets_disabled",
        render_bottom_popup(&app.chat_widget, /*width*/ 80)
    );
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.chat_widget
            .selected_index_for_present_view("usage-menu"),
        Some(0)
    );
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(
            event,
            AppEvent::RefreshRateLimits { .. } | AppEvent::OpenRateLimitResetCredits
        ));
    }
    let request_id = app.chat_widget.show_rate_limit_reset_loading_popup();
    app.refresh_rate_limits(&server, RateLimitRefreshOrigin::ResetPicker { request_id });
    let event = rx.try_recv()?;
    assert!(matches!(
        event,
        AppEvent::RateLimitsLoaded { result: Err(_), .. }
    ));
    app.handle_event(&mut tui, &mut server, event).await?;
    insta::assert_snapshot!(
        "custom_provider_pending_reset_completed",
        render_bottom_popup(&app.chat_widget, /*width*/ 80)
    );
    assert!(!app.rate_limit_refresh_state.has_pending_recovery());
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn custom_provider_remote_usage_enabled_survives_resume_and_config_reload() -> Result<()> {
    assert_remote_usage_setting_survives_resume_and_config_reload(UsageSetting::Enabled).await
}

#[tokio::test]
async fn custom_provider_remote_usage_disabled_survives_resume_and_config_reload() -> Result<()> {
    assert_remote_usage_setting_survives_resume_and_config_reload(UsageSetting::Disabled).await
}

#[tokio::test]
async fn custom_provider_remote_usage_missing_survives_resume_and_config_reload() -> Result<()> {
    assert_remote_usage_setting_survives_resume_and_config_reload(UsageSetting::Missing).await
}

enum UsageSetting {
    Enabled,
    Disabled,
    Missing,
}

async fn assert_remote_usage_setting_survives_resume_and_config_reload(
    setting: UsageSetting,
) -> Result<()> {
    let server_usage_flag = match setting {
        UsageSetting::Enabled => Some(true),
        UsageSetting::Disabled => Some(false),
        UsageSetting::Missing => None,
    };
    let server_supports_usage = server_usage_flag.unwrap_or(false);
    let home = tempdir()?;
    let usage_setting = server_usage_flag
        .map(|flag| format!("supports_usage = {flag}"))
        .unwrap_or_default();
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            r#"
model = "gpt-5.6-sol"
model_provider = "custom"
[model_providers.custom]
name = "Custom"
{usage_setting}
"#
        ),
    )?;
    let server_config = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .fallback_cwd(Some(home.path().to_path_buf()))
        .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
        .build()
        .await?;
    let (mut server, requests, proxy) = start_recording_app_server_with_history(
        &server_config,
        HistoryCapabilities::Current,
        /*blocked_thread_list*/ None,
        /*failed_thread_name*/ None,
        crate::app_server_session::ThreadParamsMode::Remote,
        LoaderOverrides::without_managed_config_for_tests(),
    )
    .await?;
    let (mut app, mut rx, _ops) = make_test_app_with_channels().await;
    app.config.model = Some("gpt-5.6-sol".into());
    app.config.model_provider.supports_usage = !server_supports_usage;
    app.app_server_target = AppServerTarget::Remote {
        endpoint: crate::RemoteAppServerEndpoint::WebSocket {
            websocket_url: "ws://127.0.0.1:1".into(),
            auth_token: None,
        },
    };
    let bootstrap = server.bootstrap(&app.config).await?;
    server.sync_provider_usage(&mut app.config);
    app.chat_widget.sync_provider_usage(&app.config);
    app.chat_widget.requires_openai_auth = bootstrap.requires_openai_auth;
    assert_eq!(
        app.config.model_provider.supports_usage,
        server_supports_usage
    );
    let thread_id = ThreadId::from_string(
        &app_test_support::create_fake_rollout(
            home.path(),
            "2026-01-01T00-00-00",
            "2026-01-01T00:00:00Z",
            "custom prompt",
            Some("custom"),
            /*git_info*/ None,
        )
        .expect("create custom provider rollout"),
    )?;
    let resumed = server
        .resume_thread(
            &app.local_settings,
            app.config.clone(),
            thread_id,
            crate::app_server_session::ResumeModelSettings::RestoreFromThread,
        )
        .await?;
    app.chat_widget.handle_thread_session(resumed.session);
    app.refresh_in_memory_config_from_disk().await?;
    assert_eq!(
        app.config.model_provider.supports_usage,
        server_supports_usage
    );
    while rx.try_recv().is_ok() {}
    app.chat_widget.insert_str("/status");
    app.chat_widget
        .handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::InsertHistoryCell(_))),
        "{events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::StatusCommand { .. },
                }
            ))
            .count(),
        usize::from(server_supports_usage)
    );
    assert_eq!(recorded_params(&requests, "thread/resume").len(), 1);
    server.shutdown().await?;
    proxy.abort();
    Ok(())
}
