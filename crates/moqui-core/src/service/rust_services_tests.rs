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
        "moqui_core::service::call_media::get_call_audio",
        "moqui_core::service::call_media::get_caption_speaker",
        "moqui_core::service::call_media::get_phone_voicemail_audio",
        "moqui_core::service::call_media::make_ice_servers",
        "moqui_core::service::call_media::send_media_control",
        "moqui_core::service::call_media::send_sip_control",
        "moqui_core::service::call_media::store_edge_call_audio",
        "moqui_core::service::call_media::store_recording_file",
        "moqui_core::service::call_media::store_voicemail_file",
        "moqui_core::service::call_media::transcribe_caption_audio",
        "moqui_core::service::chat_keys::derive_store_key",
        "moqui_core::service::protocol_admin::get_protocol_facts",
        "moqui_core::service::protocol_admin::probe_protocol_target",
        "moqui_core::service::recent_auth::check_recent_auth",
        "moqui_core::service::recent_auth::reauthenticate_user",
        "moqui_core::service::recent_auth::send_reauth_code",
        "moqui_core::service::chat::check_record_visible",
        "moqui_core::service::chat::get_external_users",
        "moqui_core::service::chat::push_signal",
        "moqui_core::service::chat_bridge::check_bridge_token",
        "moqui_core::service::chat_bridge::edge::apply_edge_webhook",
        "moqui_core::service::chat_bridge::edge::store_edge_media",
        "moqui_core::service::chat_bridge::get_bridge_capabilities",
        "moqui_core::service::chat_bridge::get_local_actors",
        "moqui_core::service::chat_bridge::run_bridge_deliveries",
        "moqui_core::service::chat_bridge::wake_bridge_delivery",
        "moqui_core::service::chat_bridge::web::send_bridge_direct",
        "moqui_core::service::chat_bridge::web::store_bridge_media",
        "moqui_core::service::chat_link::extract_link_urls",
        "moqui_core::service::chat_link::fetch_link_page",
        "moqui_core::service::chat_migrate::drop_pre_c1_backups",
        "moqui_core::service::chat_migrate::get_pre_c1_backups",
        "moqui_core::service::chat_migrate::migrate_chat_actors",
        "moqui_core::service::contacts::filter_contact_addresses",
        "moqui_core::service::content::gc_content_blobs",
        "moqui_core::service::content::migrate_db_resource_blobs",
        "moqui_core::service::edge::add_edge_host",
        "moqui_core::service::edge::apply_edge_setup",
        "moqui_core::service::edge::get_edge_offer",
        "moqui_core::service::edge::receive_edge_echo",
        "moqui_core::service::edge::test_edge_setup",
        "moqui_core::service::edge::check_edge_node",
        "moqui_core::service::edge::create_pairing_code",
        "moqui_core::service::edge::enqueue_item",
        "moqui_core::service::edge::get_edge_clients",
        "moqui_core::service::edge::get_edge_hosts",
        "moqui_core::service::edge::get_link_status",
        "moqui_core::service::edge::get_queue_status",
        "moqui_core::service::edge::get_replica_values",
        "moqui_core::service::edge::pair_edge_node",
        "moqui_core::service::edge::pause_queue_delivery",
        "moqui_core::service::edge::ping_edge_client",
        "moqui_core::service::edge::remove_edge_host",
        "moqui_core::service::edge::resync_replication",
        "moqui_core::service::edge::revoke_edge_client",
        "moqui_core::service::edge::unpair_edge_node",
        "moqui_core::service::edge_claims::check_replica_claim",
        "moqui_core::service::edge_claims::get_replica_conflicts",
        "moqui_core::service::edge_claims::recheck_replica_conflicts",
        "moqui_core::service::external_agent::request",
        "moqui_core::service::images::convert_image",
        "moqui_core::service::images::get_rich_text_image",
        "moqui_core::service::images::image_reader_file_suffixes",
        "moqui_core::service::images::image_reader_mime_types",
        "moqui_core::service::images::image_writer_file_suffixes",
        "moqui_core::service::images::image_writer_mime_types",
        "moqui_core::service::images::upload_rich_text_image",
        "moqui_core::service::images::write_image_service",
        "moqui_core::service::shell::call_providers",
        "moqui_core::service::shell::get_appearance",
        "moqui_core::service::shell::get_appearance_policy",
        "moqui_core::service::shell::get_color_scheme_css",
        "moqui_core::service::shell::get_providers",
        "moqui_core::service::shell::get_shell_counts",
        "moqui_core::service::shell::publish_shell_counts",
        "moqui_core::service::shell::set_user_screen_theme",
        "moqui_core::service::sign_in_policy::get_sign_in_policy_values",
        "moqui_core::service::sip_credential::make_sip_credential",
        "moqui_core::service::sign_in_policy::get_sign_in_policy_values",
        "moqui_core::service::sip_credential::claim_sip_digest_nonce",
        "moqui_core::service::sip_credential::compute_sip_digest_response",
        "moqui_core::service::speech::get_speech_provider",
        "moqui_core::service::speech::transcribe_audio",
        "moqui_core::service::spreadsheet::load_spreadsheet_entity",
        "moqui_core::service::spreadsheet::read_spreadsheet",
        "moqui_core::service::sso::check_flow",
        "moqui_core::service::sso::handle_callback",
        "moqui_core::service::sso::login_user",
        "moqui_core::service::sso::logout_user",
        "moqui_core::service::sso_app::confirm_app_login",
        "moqui_core::service::sso_app::exchange_app_code",
        "moqui_core::service::sso_app::get_app_providers",
        "moqui_core::service::sso_app::issue_app_code",
        "moqui_core::service::sso_app::start_app_login",
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
        "moqui_core::service::user_keys::check_plain_password",
        "moqui_core::service::user_keys::create_access_token",
        "moqui_core::service::user_keys::create_app_password",
        "moqui_core::service::user_keys::create_ssh_key",
        "moqui_core::service::user_keys::delete_ssh_key",
        "moqui_core::service::user_keys::get_app_passwords",
        "moqui_core::service::user_keys::get_device_keys",
        "moqui_core::service::user_keys::get_scram_credentials",
        "moqui_core::service::user_keys::renew_device_key",
        "moqui_core::service::user_keys::revoke_access_token",
        "moqui_core::service::user_keys::revoke_app_password",
        "moqui_core::service::user_keys::revoke_device_key",
        "moqui_core::service::web_push::delete_subscription",
        "moqui_core::service::web_push::get_device_status",
        "moqui_core::service::web_push::get_status",
        "moqui_core::service::web_push::register_device",
        "moqui_core::service::web_push::send_web_push",
        "moqui_core::service::web_push::store_subscription",
        "moqui_core::service::web_push::unregister_device",
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
    assert_eq!(
        super::missing_feature("moqui_tools::service::get_capabilities"),
        Some("tools")
    );
    assert_eq!(super::missing_feature("moqui_dav::elfinder::run_command"), Some("dav"));
    assert_eq!(
        super::missing_feature("moqui_devkit::service::restart_server"),
        Some("devkit")
    );
    // moqui-mail is smtp, with the receive half its imap
    assert_eq!(super::missing_feature("moqui_mail::send_message"), Some("smtp"));
    assert_eq!(super::missing_feature("moqui_mail::push_remote_changes"), Some("smtp"));
    assert_eq!(super::missing_feature("moqui_mail::sync_account"), Some("imap"));
    assert!(!crate::build_info::core_features().contains(&"smtp"));
    // the host's load runs, its test-client feature
    assert_eq!(
        super::missing_feature("moqui_host::load_runner::find_load_runs"),
        Some("test-client")
    );
}
