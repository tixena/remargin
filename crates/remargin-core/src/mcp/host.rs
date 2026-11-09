//! The synchronous stdio host of the generated server: one line in, one answer out.

use core::future::{Future, ready};
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::io::{self, BufRead as _, Write as _};
use std::path::Path;

use anyhow::Context as _;
use os_shim::System;
use serde_json::Value;

use crate::config::ResolvedConfig;
use crate::config::identity::IdentityFlags;
use crate::config::permissions::resolve::resolve_permissions;
use crate::mcp::context::RequestContext;
use crate::mcp::dispatcher::{self, McpClientGone, McpEventWriter, McpInFlight};
use crate::mcp::handlers::Server;

/// Writes each message the transport sends beside an answer as one line of stdout.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct StdoutLines;

/// Drops every message: for a caller that reads the answer alone.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Silent;

impl McpEventWriter for StdoutLines {
    fn keep_alive(&self) -> impl Future<Output = Result<(), McpClientGone>> + Send {
        ready(Ok(()))
    }

    fn write(&self, message: String) -> impl Future<Output = Result<(), McpClientGone>> + Send {
        ready(write_line(&message).map_err(|_err| McpClientGone::new()))
    }
}

impl McpEventWriter for Silent {
    fn keep_alive(&self) -> impl Future<Output = Result<(), McpClientGone>> + Send {
        ready(Ok(()))
    }

    fn write(&self, _message: String) -> impl Future<Output = Result<(), McpClientGone>> + Send {
        ready(Ok(()))
    }
}

fn write_line(message: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{message}")?;
    stdout.flush()
}

/// Polls `work` until it is ready: no handler here waits on anything.
pub fn block_on<F>(work: F) -> F::Output
where
    F: Future,
{
    let mut pinned = pin!(work);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = pinned.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

/// The size `report_spill` infers from: the UTF-8 bytes of the text, data and resource blobs an
/// answer's `content` carries.
fn content_size(answer: &str) -> Option<usize> {
    let parsed: Value = serde_json::from_str(answer).ok()?;
    let content = parsed.get("result")?.get("content")?.as_array()?;
    Some(
        content
            .iter()
            .flat_map(|item| {
                [
                    item.get("text"),
                    item.get("data"),
                    item.get("resource")
                        .and_then(|resource| resource.get("blob")),
                ]
            })
            .flatten()
            .filter_map(Value::as_str)
            .map(str::len)
            .sum(),
    )
}

/// Answers one line of the wire, and records the size of the answer on `server`.
pub fn serve_line<W>(
    server: &Server,
    context: RequestContext<'_>,
    line: &str,
    writer: &W,
    in_flight: &McpInFlight,
) -> Option<String>
where
    W: McpEventWriter + Sync,
{
    let answer = block_on(dispatcher::answer(
        line,
        server,
        |_call| context,
        writer,
        in_flight,
    ))?;
    if let Some(size) = content_size(&answer) {
        server.note_response_size(size);
    }
    Some(answer)
}

/// Answers one request against a configuration the caller resolved, as `serve_line` does for
/// each line of stdin.
///
/// # Errors
///
/// Returns an error if the permissions of `base_dir` cannot be resolved.
pub fn process_request(
    server: &Server,
    system: &dyn System,
    base_dir: &Path,
    config: ResolvedConfig,
    request_json: &str,
) -> anyhow::Result<Option<String>> {
    let permissions = resolve_permissions(system, base_dir)?;
    let context = RequestContext {
        base_dir,
        config,
        permissions,
        system,
    };
    Ok(serve_line(
        server,
        context,
        request_json,
        &Silent,
        &McpInFlight::default(),
    ))
}

/// Runs the generated server over stdin and stdout until stdin closes.
///
/// `startup_flags` and `startup_assets_dir` carry the identity declaration and assets directory
/// given on the command line; they are applied to every request, and the configuration and the
/// permissions are resolved again for each, so a changed `.remargin.yaml` takes effect without a
/// restart.
///
/// # Errors
///
/// Returns an error if the configuration or the permissions cannot be resolved, or stdin or
/// stdout fail.
pub fn run(
    system: &dyn System,
    base_dir: &Path,
    startup_flags: &IdentityFlags,
    startup_assets_dir: Option<&str>,
) -> anyhow::Result<()> {
    let server = Server::new();
    let in_flight = McpInFlight::default();
    for raw_line in io::stdin().lock().lines() {
        let line = raw_line.context("reading from stdin")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let config = ResolvedConfig::resolve(system, base_dir, startup_flags, startup_assets_dir)?;
        let permissions = resolve_permissions(system, base_dir)?;
        let context = RequestContext {
            base_dir,
            config,
            permissions,
            system,
        };
        if let Some(answer) = serve_line(&server, context, trimmed, &StdoutLines, &in_flight) {
            write_line(&answer).context("writing to stdout")?;
        }
    }
    Ok(())
}
