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

#[test]
fn registry_keys_are_the_function_paths() {
    let map = super::build();
    let mut keys: Vec<&str> = map.keys().copied().collect();
    keys.sort_unstable();
    let mut expected = vec![
        "moqui_core::service::backup::backup_embedded_database",
        "moqui_core::service::chat::check_record_visible",
        "moqui_core::service::chat::get_external_users",
        "moqui_core::service::chat::push_signal",
        "moqui_core::service::chat_link::extract_link_urls",
        "moqui_core::service::chat_link::fetch_link_page",
        "moqui_core::service::contacts::filter_contact_addresses",
        "moqui_core::service::content::gc_content_blobs",
        "moqui_core::service::devtools::apply_xml_ops",
        "moqui_core::service::devtools::find_artifacts",
        "moqui_core::service::devtools::get_artifact",
        "moqui_core::service::devtools::get_artifact_at",
        "moqui_core::service::devtools::get_artifact_extensions",
        "moqui_core::service::devtools::get_artifact_refs",
        "moqui_core::service::devtools::get_schema_digest",
        "moqui_core::service::devtools::get_xml_outline",
        "moqui_core::service::devtools::rebuild_artifact_index",
        "moqui_core::service::devtools::restart_server",
        "moqui_core::service::content::migrate_db_resource_blobs",
        "moqui_core::service::external_agent::request",
        "moqui_core::service::images::convert_image",
        "moqui_core::service::images::get_rich_text_image",
        "moqui_core::service::images::image_reader_file_suffixes",
        "moqui_core::service::images::image_reader_mime_types",
        "moqui_core::service::images::image_writer_file_suffixes",
        "moqui_core::service::images::image_writer_mime_types",
        "moqui_core::service::images::upload_rich_text_image",
        "moqui_core::service::images::write_image_service",
        "moqui_core::service::shell::get_shell_counts",
        "moqui_core::service::shell::publish_shell_counts",
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
        "moqui_core::service::user_keys::create_access_token",
        "moqui_core::service::user_keys::create_app_password",
        "moqui_core::service::user_keys::create_ssh_key",
        "moqui_core::service::user_keys::delete_ssh_key",
        "moqui_core::service::user_keys::get_app_passwords",
        "moqui_core::service::user_keys::revoke_access_token",
        "moqui_core::service::user_keys::revoke_app_password",
    ];
    expected.sort_unstable();
    // A feature this build leaves out registers nothing; its locations resolve through missing_feature.
    expected.retain(|location| super::missing_feature(location).is_none());
    assert_eq!(keys, expected);
}

/// A framework crate's service this binary does not link loads and reports its feature missing; core's tests link no
/// framework crate.
#[test]
fn an_unlinked_framework_crate_service_reports_its_feature() {
    assert_eq!(super::missing_feature("moqui_git::service::get_refs"), Some("git"));
    assert!(!crate::build_info::core_features().contains(&"git"));
    assert_eq!(
        super::missing_feature("moqui_collab::service::save_document"),
        Some("collab")
    );
    assert_eq!(super::missing_feature("moqui_dav::elfinder::run_command"), Some("dav"));
    // moqui-mail is smtp, with the receive half its imap
    assert_eq!(super::missing_feature("moqui_mail::send_message"), Some("smtp"));
    assert_eq!(super::missing_feature("moqui_mail::push_remote_changes"), Some("smtp"));
    assert_eq!(super::missing_feature("moqui_mail::sync_account"), Some("imap"));
    assert!(!crate::build_info::core_features().contains(&"smtp"));
}
