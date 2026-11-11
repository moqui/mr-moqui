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

//! Network printing, the `moqui-cups` component. Core has no HTTP stack; the host installs a [`PrintClient`] that speaks
//! IPP to a CUPS server (or any IPP printer). The print services (`crate::service::tools`) keep the `PrintJob` and
//! `NetworkPrinter` records and call it. With no client installed a call reports that network printing is not
//! installed.

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::sync::InstallSlot;

/// The message when no client is installed.
pub const PRINTING_UNAVAILABLE: &str = "Network printing support not installed (no print client in this build)";

/// A print server: a `NetworkPrinter`'s `serverHost` and `serverPort`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintServer {
    pub host: String,
    pub port: u16,
    /// The printer's `allowPrivateNetwork` is N (a delegated administrator's on a production instance): the client
    /// sends nothing to a loopback, private or link-local address.
    pub deny_internal: bool,
}

/// A printer a server has, as cups4j's `CupsPrinter`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrinterInfo {
    pub name: String,
    pub description: String,
    pub location: String,
    /// The printer's `printer-uri-supported`, which jobs are sent to.
    pub uri: String,
}

/// One document to print, as cups4j's `PrintJob.Builder`.
#[derive(Clone, Debug, Default)]
pub struct PrintDocument {
    pub bytes: Vec<u8>,
    /// The MIME type; empty lets the server detect it (`application/octet-stream`).
    pub content_type: String,
    pub user_name: Option<String>,
    pub job_name: Option<String>,
    pub copies: Option<i32>,
    /// `sides`: two-sided long edge when true, one-sided when false.
    pub duplex: Option<bool>,
    /// `page-ranges` as text, such as `1-3,5`.
    pub page_ranges: Option<String>,
    /// `orientation-requested`: portrait when true, landscape when false. cups4j sent portrait on every job; none
    /// leaves it to the queue.
    pub portrait: Option<bool>,
    /// `print-color-mode`: color when true, monochrome when false. cups4j sent monochrome (as CUPS's `output-mode`) on
    /// every job; none leaves it to the queue.
    pub color: Option<bool>,
}

/// The outcome of a print request: the job id, and the server's description when it did not take the job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintResult {
    pub job_id: Option<i32>,
    pub error: Option<String>,
}

/// A job's attributes from the server, as cups4j's `PrintJobAttributes`. Times are epoch seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobAttributes {
    /// `job-state` as the keyword cups4j's `JobStateEnum` printed: pending, pending-held, processing,
    /// processing-stopped, canceled, aborted, completed.
    pub state: String,
    pub complete_time: Option<i64>,
    pub create_time: Option<i64>,
    pub job_url: Option<String>,
    pub pages_printed: Option<i32>,
    pub printer_url: Option<String>,
    /// `job-k-octets`.
    pub size: Option<i32>,
}

/// The IPP work, installed by the host.
pub trait PrintClient: Send + Sync {
    /// The printers of a server (CUPS-Get-Printers).
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot be reached or refuses the request.
    fn printers(&self, server: &PrintServer) -> Result<Vec<PrinterInfo>, Error>;

    /// The server's default printer (CUPS-Get-Default), if it has one.
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot be reached.
    fn default_printer(&self, server: &PrintServer) -> Result<Option<PrinterInfo>, Error>;

    /// Send a document to a printer (Print-Job).
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot be reached; a refused job is a [`PrintResult`] with an error.
    fn print(
        &self,
        server: &PrintServer,
        printer: &PrinterInfo,
        document: &PrintDocument,
    ) -> Result<PrintResult, Error>;

    /// A job's attributes (Get-Job-Attributes).
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot be reached or does not know the job.
    fn job_attributes(&self, server: &PrintServer, job_id: i32) -> Result<JobAttributes, Error>;

    /// Hold, release, or cancel a job: true when the server did it.
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot be reached.
    fn job_action(
        &self,
        server: &PrintServer,
        printer: &PrinterInfo,
        job_id: i32,
        action: JobAction,
    ) -> Result<bool, Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobAction {
    Hold,
    Release,
    Cancel,
}

static CLIENT: InstallSlot<dyn PrintClient> = InstallSlot::new();

/// Use `client` for network printing in this process.
pub fn install_print_client(client: Arc<dyn PrintClient>) {
    CLIENT.install(client);
}

/// Remove the installed client.
pub fn clear_print_client() {
    CLIENT.clear();
}

pub(crate) fn client() -> Result<Arc<dyn PrintClient>, Error> {
    CLIENT.get().ok_or_else(|| Error::new(PRINTING_UNAVAILABLE))
}

/// Tests that install a client hold this, so they do not see each other's.
#[cfg(test)]
pub(crate) fn print_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    moqui_lib::sync::lock(&LOCK)
}
