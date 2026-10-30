/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! `git-upload-pack '<repo>'` and `git-receive-pack '<repo>'` as SSH `exec` commands: the channel's bytes go to and
//! from `moqui_git::server::serve_ssh` on a job of the git pool, as the logged-in user. Its messages
//! go on the channel's stderr, then its exit status, end of file and close. `GIT_PROTOCOL` from the channel's `env`
//! request selects protocol v2 (`ssh -o SendEnv=GIT_PROTOCOL`, which git sets up itself).

use std::sync::{Arc, Weak};

use moqui_core::context::ExecutionContextFactory;
use moqui_git::server;
use russh::server::{Handle, Msg};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::git_bridge::{ChannelReader, ChannelWriter, QUEUED};

/// Serve one git command on `channel` and close it.
pub(super) async fn serve_channel(
    channel: Channel<Msg>,
    id: ChannelId,
    handle: Handle,
    factory: Weak<ExecutionContextFactory>,
    username: String,
    command: String,
    git_protocol: Option<String>,
) {
    let (mut reader, mut writer) = tokio::io::split(channel.into_stream());
    let (in_tx, in_rx) = mpsc::channel(QUEUED);
    let (out_tx, mut out_rx) = mpsc::channel(QUEUED);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<(u32, Vec<u8>)>();
    let Some(strong) = factory.upgrade() else {
        let _ = handle.close(id).await;
        return;
    };
    let submitted = server::pool(&strong).try_submit(Arc::clone(&strong), move || {
        let mut input = ChannelReader::new(in_rx);
        let mut output = ChannelWriter::new(out_tx);
        let mut stderr = Vec::new();
        let status = server::serve_ssh(
            &username,
            &command,
            git_protocol.as_deref(),
            &mut input,
            &mut output,
            &mut stderr,
        );
        let _ = std::io::Write::flush(&mut output);
        drop(output);
        let _ = done_tx.send((status, stderr));
        Ok(())
    });
    drop(strong);
    if submitted.is_err() {
        let _ = handle
            .extended_data(id, 1, b"fatal: the git server is busy; try again\n".to_vec())
            .await;
        let _ = handle.exit_status_request(id, 128).await;
        let _ = handle.eof(id).await;
        let _ = handle.close(id).await;
        return;
    }
    // the client's bytes in, until it closes its side or the job stops reading
    let feed = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if in_tx
                        .send(Ok(bytes::Bytes::copy_from_slice(&buf[..count])))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    while let Some(chunk) = out_rx.recv().await {
        let Ok(bytes) = chunk else { break };
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break;
        }
    }
    let (status, stderr) = done_rx.await.unwrap_or((128, b"fatal: the git job stopped\n".to_vec()));
    feed.abort();
    if !stderr.is_empty() {
        let _ = handle.extended_data(id, 1, stderr).await;
    }
    let _ = handle.exit_status_request(id, status).await;
    let _ = handle.eof(id).await;
    let _ = handle.close(id).await;
}
