//! Fetching and updating subscriptions.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use uniray_core::hwid;
use uniray_core::model::{new_id, Subscription};
use uniray_core::subscription::{self, Parsed};

use crate::app::{emit_state, App};
use crate::error::{CmdError, CmdResult};
use crate::logs::now_secs;

/// Update interval when the panel does not advertise one.
const DEFAULT_INTERVAL_HOURS: u32 = 12;
const AUTO_UPDATE_CHECK: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Servers imported.
    pub added: usize,
    /// Entries that could not be imported, with the reason.
    pub errors: Vec<String>,
}

/// Requests the subscription, identifying the device unless that is turned
/// off. A panel with Remnawave's HWID device limit answers 404 without it.
async fn fetch(app: &AppHandle, url: &str) -> CmdResult<Parsed> {
    let state = app.state::<App>();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(CmdError::new("invalidUrl", "a subscription URL must start with http:// or https://"));
    }
    let send_hwid = state.store.lock().unwrap().data.settings.send_hwid;
    let headers = hwid::request_headers(&state.device.lock().unwrap(), send_hwid);
    let hwid_sent = headers.iter().any(|(name, _)| *name == "x-hwid");

    let request = |client: &reqwest::Client| {
        let mut request = client.get(url);
        for (name, value) in &headers {
            request = request.header(*name, value);
        }
        request.send()
    };
    let response = match request(&state.http).await {
        Ok(response) => response,
        Err(direct_err) => {
            // The panel may be reachable only through the VPN.
            let mixed_port = state.connection.lock().unwrap().mixed_port;
            let settings = state.store.lock().unwrap().data.settings.clone();
            let proxied = match mixed_port.and_then(|port| proxied_client(&settings, port)) {
                Some(client) => request(&client).await.ok(),
                None => None,
            };
            proxied.ok_or_else(|| CmdError::network(describe(&direct_err)))?
        }
    };

    let status = response.status().as_u16();
    let response_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (name.as_str().to_ascii_lowercase(), String::from_utf8_lossy(value.as_bytes()).into_owned())
        })
        .collect();
    let body = response
        .text()
        .await
        .map_err(|err| CmdError::network(describe(&err)))?;
    Ok(subscription::parse_response(status, &response_headers, &body, hwid_sent)?)
}

fn proxied_client(settings: &uniray_core::model::Settings, mixed_port: u16) -> Option<reqwest::Client> {
    let proxy = reqwest::Proxy::all(crate::monitor::local_proxy_url(settings, mixed_port)).ok()?;
    reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(30))
        .build()
        .ok()
}

/// reqwest's own message stops at "error sending request"; the cause is in the
/// source chain.
pub(crate) fn describe(err: &reqwest::Error) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).to_owned()
}

/// Adds a subscription and imports its servers. Nothing is stored when the
/// first fetch fails.
pub async fn add(app: &AppHandle, url: &str) -> CmdResult<ImportReport> {
    let url = url.trim();
    let state = app.state::<App>();
    let existing = {
        let store = state.store.lock().unwrap();
        store.data.subscriptions.iter().find(|sub| sub.url == url).map(|sub| sub.id.clone())
    };
    if let Some(id) = existing {
        return update(app, &id).await;
    }

    let parsed = fetch(app, url).await?;
    let mut subscription = Subscription {
        id: new_id(),
        name: host_of(url),
        url: url.to_owned(),
        ..Default::default()
    };
    let report = ImportReport { added: parsed.servers.len(), errors: parsed.errors };
    parsed.meta.apply(&mut subscription, now_secs());
    let servers = subscription::reconcile(&[], parsed.servers, &subscription.id);
    state
        .update(|store| {
            store.data.subscriptions.push(subscription);
            store.data.servers.extend(servers);
        })
        .map_err(CmdError::io)?;
    emit_state(app);
    Ok(report)
}

/// Refetches a subscription and replaces its servers. On failure the servers
/// stay and the error is recorded on the subscription.
pub async fn update(app: &AppHandle, id: &str) -> CmdResult<ImportReport> {
    let state = app.state::<App>();
    let url = {
        let store = state.store.lock().unwrap();
        store.data.subscriptions.iter().find(|sub| sub.id == id).map(|sub| sub.url.clone())
    }
    .ok_or_else(|| CmdError::new("notFound", "no such subscription"))?;

    let outcome = fetch(app, &url).await;
    let result = state
        .update(|store| {
            let data = &mut store.data;
            let Some(subscription) = data.subscriptions.iter_mut().find(|sub| sub.id == id) else {
                return Err(CmdError::new("notFound", "the subscription was removed"));
            };
            match outcome {
                Ok(parsed) => {
                    let report = ImportReport { added: parsed.servers.len(), errors: parsed.errors };
                    parsed.meta.apply(subscription, now_secs());
                    // Replace the subscription's servers in place, keeping
                    // the position of the group in the list.
                    let of_subscription = |server: &uniray_core::model::Server| {
                        server.subscription_id.as_deref() == Some(id)
                    };
                    let position = data.servers.iter().position(of_subscription).unwrap_or(data.servers.len());
                    let (old, mut rest): (Vec<_>, Vec<_>) = data.servers.drain(..).partition(of_subscription);
                    let position = position.min(rest.len());
                    let new = subscription::reconcile(&old, parsed.servers, id);
                    rest.splice(position..position, new);
                    data.servers = rest;
                    Ok(report)
                }
                Err(err) => {
                    subscription.last_error = Some(err.message.clone());
                    Err(err)
                }
            }
        })
        .map_err(CmdError::io)?;
    emit_state(app);
    result
}

pub async fn remove(app: &AppHandle, id: &str) -> CmdResult<()> {
    app.state::<App>()
        .update(|store| {
            store.data.subscriptions.retain(|sub| sub.id != id);
            store.data.servers.retain(|server| server.subscription_id.as_deref() != Some(id));
        })
        .map_err(CmdError::io)?;
    emit_state(app);
    Ok(())
}

/// Keeps subscriptions fresh in the background for as long as the app runs.
pub fn spawn_auto_update(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let due: Vec<String> = {
                let state = app.state::<App>();
                let store = state.store.lock().unwrap();
                let now = now_secs();
                store
                    .data
                    .subscriptions
                    .iter()
                    .filter(|sub| {
                        let interval = sub.update_interval_hours.unwrap_or(DEFAULT_INTERVAL_HOURS);
                        sub.updated_at.is_none_or(|at| now - at >= i64::from(interval) * 3600)
                    })
                    .map(|sub| sub.id.clone())
                    .collect()
            };
            for id in due {
                // Failures are recorded on the subscription and shown there.
                let _ = update(&app, &id).await;
            }
            tokio::time::sleep(AUTO_UPDATE_CHECK).await;
        }
    });
}
