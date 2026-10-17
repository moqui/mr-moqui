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

//! Function registry for `type="rust"` services. Each function registers under its Rust path, down to the function
//! name (`moqui_core::service::mail::send_template`). The service element's `location` is that path.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::entity::EntityFacade;

/// A Rust service body. The service name stays on the definition; the pointer is the `location`.
pub(crate) type RustServiceFn =
    fn(&BTreeMap<String, Value>, &EntityFacade) -> Result<BTreeMap<String, Value>, LibError>;

/// Register `$function` under `module_path!() + "::" + its name`. A second insert of that path is an error.
macro_rules! rust_service {
    ($map:expr, $function:ident) => {{
        let location: &'static str = concat!(module_path!(), "::", stringify!($function));
        debug_assert_eq!(location, ::std::any::type_name_of_val(&$function));
        if $map.insert(location, $function).is_some() {
            Err(format!("duplicate rust service {location}"))
        } else {
            Ok(())
        }
    }};
}

static REGISTRY: OnceLock<BTreeMap<&'static str, RustServiceFn>> = OnceLock::new();

/// The function registered at `location`, once every module has registered.
pub(crate) fn get(location: &str) -> Option<RustServiceFn> {
    registry().get(location).copied()
}

fn registry() -> &'static BTreeMap<&'static str, RustServiceFn> {
    REGISTRY.get_or_init(build)
}

fn build() -> BTreeMap<&'static str, RustServiceFn> {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    for result in [
        super::backup::register(&mut map),
        super::mail::register(&mut map),
        super::system_sync::register(&mut map),
        super::elfinder::register(&mut map),
        super::sso::register(&mut map),
        super::tools::register(&mut map),
        super::images::register(&mut map),
        super::spreadsheet::register(&mut map),
    ] {
        if let Err(error) = result {
            errors.push(error);
        }
    }
    assert!(errors.is_empty(), "rust service registry: {}", errors.join("; "));
    map
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_keys_are_the_function_paths() {
        let map = super::build();
        let mut keys: Vec<&str> = map.keys().copied().collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "moqui_core::service::backup::backup_embedded_database",
                "moqui_core::service::elfinder::run_command",
                "moqui_core::service::images::convert_image",
                "moqui_core::service::images::image_reader_file_suffixes",
                "moqui_core::service::images::image_reader_mime_types",
                "moqui_core::service::images::image_writer_file_suffixes",
                "moqui_core::service::images::image_writer_mime_types",
                "moqui_core::service::images::write_image_service",
                "moqui_core::service::mail::poll_server",
                "moqui_core::service::mail::send_message",
                "moqui_core::service::mail::send_template",
                "moqui_core::service::spreadsheet::load_spreadsheet_entity",
                "moqui_core::service::spreadsheet::read_spreadsheet",
                "moqui_core::service::sso::handle_callback",
                "moqui_core::service::sso::login_user",
                "moqui_core::service::sso::logout_user",
                "moqui_core::service::system_sync::entity_sync::get_data",
                "moqui_core::service::system_sync::entity_sync::include_list",
                "moqui_core::service::system_sync::entity_sync::internal_run",
                "moqui_core::service::system_sync::entity_sync::put_data",
                "moqui_core::service::system_sync::entity_sync::run_sync",
                "moqui_core::service::system_sync::message::send_rest",
                "moqui_core::service::system_sync::sftp::poll",
                "moqui_core::service::system_sync::sftp::send",
                "moqui_core::service::tools::cancel_print_job",
                "moqui_core::service::tools::get_print_job_details",
                "moqui_core::service::tools::get_server_printers",
                "moqui_core::service::tools::hold_print_job",
                "moqui_core::service::tools::print_document",
                "moqui_core::service::tools::print_resource_document",
                "moqui_core::service::tools::print_screen_document",
                "moqui_core::service::tools::release_print_job",
                "moqui_core::service::tools::rewrite_encrypted_fields",
                "moqui_core::service::tools::send_document_notifications",
                "moqui_core::service::tools::send_print_job_internal",
            ]
        );
    }
}
