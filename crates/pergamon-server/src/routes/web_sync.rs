// SPDX-License-Identifier: AGPL-3.0-only

//! Progressive protected forms; passwords/recovery never become URL or history state.

use askama::Template;
use axum::extract::{Extension, Form, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};

use crate::{
    onboarding::{SyncCommand, SyncSnapshot, public_error},
    operator_session::OperatorContext,
    state::AppState,
};

#[derive(Template)]
#[template(path = "sync.html")]
struct SyncTemplate {
    status: SyncSnapshot,
    csrf: String,
    revision: i64,
    phase: String,
    error: String,
    pending: u64,
}

#[derive(Template)]
#[template(path = "_sync_status.html")]
struct StatusTemplate {
    status: SyncSnapshot,
    csrf: String,
    revision: i64,
    pending: u64,
}

#[derive(Template)]
#[template(path = "sync_recovery.html")]
struct RecoveryTemplate {
    code: String,
    content_id: String,
    csrf: String,
    revision: i64,
}

fn service(state: &AppState) -> anyhow::Result<std::sync::Arc<crate::onboarding::SyncService>> {
    state
        .sync
        .clone()
        .ok_or_else(|| anyhow::anyhow!("sync service is not configured"))
}

async fn view(
    state: AppState,
    context: OperatorContext,
    error: String,
    code: StatusCode,
) -> Response {
    let data = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let status = service(&state)?.snapshot()?;
        let revision = status.setup.as_ref().map_or(0, |s| s.revision);
        let phase = if status.needs_login
            && status
                .binding
                .as_ref()
                .is_some_and(|b| b.state == "pending")
        {
            "credentials".into()
        } else {
            status.setup.as_ref().map_or_else(
                || {
                    if status.binding.as_ref().is_some_and(|b| b.state == "active") {
                        "active".into()
                    } else {
                        String::new()
                    }
                },
                |s| s.phase.clone(),
            )
        };
        let pending = state
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("library lock poisoned"))?
            .pending_outbox_count()?;
        Ok(SyncTemplate {
            status,
            csrf: context.csrf,
            revision,
            phase,
            error,
            pending,
        })
    })
    .await;
    match data {
        Ok(Ok(template)) => {
            let mut response = super::web::render(&template);
            *response.status_mut() = code;
            response
        }
        Ok(Err(error)) => {
            tracing::error!(error=%error, "sync settings rendering failed");
            super::web::internal_error()
        }
        Err(error) => {
            tracing::error!(error=%error, "sync settings task failed");
            super::web::internal_error()
        }
    }
}

pub async fn settings(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
) -> Response {
    view(state, context, String::new(), StatusCode::OK).await
}

fn error_status(error: &anyhow::Error) -> StatusCode {
    match error.downcast_ref::<pergamon_sync::SyncError>() {
        Some(pergamon_sync::SyncError::RateLimited { .. }) => StatusCode::TOO_MANY_REQUESTS,
        Some(pergamon_sync::SyncError::AuthRefused { status, .. }) => {
            StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
        }
        Some(pergamon_sync::SyncError::Transport(_)) => StatusCode::SERVICE_UNAVAILABLE,
        Some(pergamon_sync::SyncError::SessionNeedsLogin { .. }) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

pub async fn action(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
    Form(command): Form<SyncCommand>,
) -> Response {
    let operation = match service(&state) {
        Ok(service) => service,
        Err(error) => {
            return view(
                state,
                context,
                public_error(&error),
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
        }
    };
    let result = tokio::task::spawn_blocking(move || operation.execute(&command)).await;
    match result {
        Ok(Ok(())) => Redirect::to("/admin/sync-remote").into_response(),
        Ok(Err(error)) => {
            let status = error_status(&error);
            let retry = error
                .downcast_ref::<pergamon_sync::SyncError>()
                .and_then(pergamon_sync::SyncError::retry_after_seconds);
            let mut response = view(state, context, public_error(&error), status).await;
            if let Some(seconds) = retry
                && let Ok(value) = seconds.to_string().parse()
            {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
        Err(error) => {
            tracing::error!(error=%error, "sync setup operation task failed");
            view(
                state,
                context,
                "Sync setup failed internally; local library data is preserved.".into(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

pub async fn status(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let status = service(&state)?.snapshot()?;
        let revision = status.setup.as_ref().map_or(0, |s| s.revision);
        let pending = state
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("library lock poisoned"))?
            .pending_outbox_count()?;
        Ok(StatusTemplate {
            status,
            csrf: context.csrf,
            revision,
            pending,
        })
    })
    .await;
    match result {
        Ok(Ok(template)) => super::web::render(&template),
        Ok(Err(error)) => {
            tracing::error!(error=%error, "sync status failed");
            super::web::internal_error()
        }
        Err(error) => {
            tracing::error!(error=%error, "sync status task failed");
            super::web::internal_error()
        }
    }
}

pub async fn recovery(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
) -> Response {
    let operation = match service(&state) {
        Ok(service) => service,
        Err(error) => {
            return view(
                state,
                context,
                public_error(&error),
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
        }
    };
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let material = operation.recovery_capture()?;
        let revision = operation
            .snapshot()?
            .setup
            .context("capture progress missing")?
            .revision;
        Ok((material, revision))
    })
    .await;
    match result {
        Ok(Ok((material, revision))) => super::web::render(&RecoveryTemplate {
            code: material.code,
            content_id: material.content_account_id,
            csrf: context.csrf,
            revision,
        }),
        Ok(Err(error)) => view(state, context, public_error(&error), StatusCode::CONFLICT).await,
        Err(error) => {
            tracing::error!(error=%error, "recovery view task failed");
            super::web::internal_error()
        }
    }
}

use anyhow::Context as _;

#[derive(serde::Deserialize)]
pub struct RevisionForm {
    revision: i64,
}

pub async fn download(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
    Form(form): Form<RevisionForm>,
) -> Response {
    let operation = match service(&state) {
        Ok(service) => service,
        Err(error) => {
            return view(
                state,
                context,
                public_error(&error),
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
        }
    };
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        if operation
            .snapshot()?
            .setup
            .as_ref()
            .map_or(0, |s| s.revision)
            != form.revision
        {
            anyhow::bail!("stale recovery form; reload capture before downloading");
        }
        operation.recovery_capture()
    })
    .await;
    match result {
        Ok(Ok(material)) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8"),
             (header::CONTENT_DISPOSITION, "attachment; filename=\"pergamon-recovery.txt\"")],
            format!("pergamon content recovery\nContent account: {}\nRecovery code: {}\n\nKeep this offline. Losing every trusted device and this material means your encrypted relay content cannot be recovered for you.\nThis is not your relay login password; resetting that password does not decrypt content.\n",
                material.content_account_id, material.code),
        ).into_response(),
        Ok(Err(error)) => view(state, context, public_error(&error), StatusCode::CONFLICT).await,
        Err(error) => {
            tracing::error!(error=%error, "recovery download task failed");
            super::web::internal_error()
        }
    }
}

pub async fn trigger(
    State(state): State<AppState>,
    Extension(context): Extension<OperatorContext>,
    Form(form): Form<RevisionForm>,
) -> Response {
    action(
        State(state),
        Extension(context),
        Form(SyncCommand::new("trigger", form.revision)),
    )
    .await
}
