use super::*;

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn framework_model() -> EntityModel {
    let root = workspace_root();
    let conf = MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).unwrap();
    EntityModel::load_directory(&conf, &root.join("runtime/base-component/framework/entity")).unwrap()
}

#[test]
fn framework_entities_and_dictionary_load() {
    let model = framework_model();
    assert!(!model.full_names().is_empty());
    let id = model.dictionary_type("id").unwrap();
    assert_eq!(id.java_type, "java.lang.String");
    assert_eq!(id.default_sql_type, "VARCHAR(40)");
    assert!(model.dictionary_type("text-fts").is_some());

    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    assert_eq!(model.entity("Enumeration").unwrap().full_name, enumeration.full_name);
    assert_eq!(enumeration.table_name, "ENUMERATION");
    assert_eq!(enumeration.entity_use, "configuration");
    assert_eq!(enumeration.cache, "true");
    assert_eq!(enumeration.sequence_bank_size, 50);
    assert_eq!(enumeration.sequence_primary_stagger, 1);
    assert_eq!(enumeration.sequence_secondary_padded_length, 2);
    assert!(enumeration.sequence_primary_prefix.is_none());
    assert!(!enumeration.sequence_primary_use_uuid);
    let stamp = enumeration.field("lastUpdatedStamp").unwrap();
    assert_eq!(stamp.field_type, "date-time");
    assert!(!stamp.is_pk);
    assert!(!stamp.not_null);
    assert_eq!(enumeration.masters[0].name, "default");
    assert_eq!(enumeration.masters[0].details[0].relationship, "type");

    let factor = model.entity("moqui.security.UserAuthcFactor").unwrap();
    let factor_id = factor.field("factorId").unwrap();
    assert!(factor_id.is_pk);
    assert!(!factor_id.not_null);
    assert!(factor.field("userId").unwrap().not_null);

    let audit = model.entity("moqui.entity.EntityAuditLog").unwrap();
    assert!(audit.create_only);
    assert!(audit.field("changedEntityName").unwrap().create_only);
    assert!(audit.field("lastUpdatedStamp").unwrap().create_only);
    let index = audit
        .indexes
        .iter()
        .find(|index| index.name == "ENTAUDLOG_PKPRIM")
        .unwrap();
    assert!(!index.unique);
    assert_eq!(index.fields, vec!["pkPrimaryValue".to_string()]);

    let detail = model.entity("moqui.basic.StatusFlowTransitionToDetail").unwrap();
    let condition = detail.entity_condition.as_ref().unwrap();
    assert_eq!(condition.order_by, vec!["sequenceNum".to_string()]);
    assert!(condition.conditions.items.is_empty());
    assert!(!condition.distinct);

    let by_alias = model.entity("artifactGroups").unwrap();
    assert_eq!(by_alias.full_name, "moqui.security.ArtifactGroup");

    let missing = model.entity("moqui.missing.NoSuch").unwrap_err();
    assert!(missing.to_string().contains("NoSuch"));
    assert!(model.entity_full("moqui.missing.NoSuch").is_none());
}

#[test]
fn artifact_authz_check_view_keeps_members() {
    let model = framework_model();
    let view = model.entity("moqui.security.ArtifactAuthzCheckView").unwrap();
    assert!(view.is_view);
    assert_eq!(view.full_name, "moqui.security.ArtifactAuthzCheckView");
    let member = view.member("AAZ").unwrap();
    assert_eq!(member.entity_name, "moqui.security.ArtifactAuthz");
    let group_member = view.member("AGM").unwrap();
    assert_eq!(group_member.relationship.as_deref(), Some("groupMembers"));
    assert_eq!(group_member.entity_name, "moqui.security.ArtifactGroupMember");
    assert_eq!(group_member.join_from_alias.as_deref(), Some("AAZ"));
    assert_eq!(
        group_member.key_map,
        vec![KeyMap {
            field_name: "artifactGroupId".to_string(),
            related: Some("artifactGroupId".to_string()),
        }]
    );
    let alias = view.alias("artifactName").unwrap();
    assert_eq!(alias.entity_alias.as_deref(), Some("AGM"));
    assert!(alias.field.is_none());
    assert_eq!(
        alias.member_entity.as_deref(),
        Some("moqui.security.ArtifactGroupMember")
    );
    assert_eq!(alias.field_type.as_deref(), Some("text-medium"));
    assert_eq!(alias.java_type, "java.lang.String");
    assert!(alias.is_pk);
    assert_eq!(
        view.pk_field_names(),
        vec![
            "artifactAuthzId",
            "artifactGroupId",
            "artifactName",
            "artifactTypeEnumId"
        ]
    );
}

#[test]
fn extend_entity_adds_a_field() {
    let root = workspace_root();
    let conf = MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).unwrap();
    let mut documents = entity_documents(&root.join("runtime/base-component/framework/entity")).unwrap();
    documents.push(
        MNode::parse_text(
            "inline-extend.xml",
            r#"<entities>
                <extend-entity entity-name="Geo" package="moqui.basic">
                    <field name="extraNote" type="text-short"/>
                </extend-entity>
            </entities>"#,
        )
        .unwrap(),
    );
    let model = EntityModel::load(&conf, &documents).unwrap();
    let geo = model.entity("moqui.basic.Geo").unwrap();
    let field = geo.field("extraNote").unwrap();
    assert_eq!(field.field_type, "text-short");
    assert!(!field.is_pk);
    assert!(!field.not_null);
    assert!(geo.field("geoId").unwrap().is_pk);
}

#[test]
fn auto_reverse_swaps_the_uom_conversion_key_map() {
    let model = framework_model();
    let conversion = model.entity("moqui.basic.UomConversion").unwrap();
    let forward = conversion.relationship("To#moqui.basic.Uom").unwrap();
    assert_eq!(forward.relationship_type, "one");
    assert!(!forward.is_auto_reverse);
    assert_eq!(forward.key_map.len(), 1);
    assert_eq!(forward.key_map[0].field_name, "toUomId");
    assert_eq!(forward.key_map[0].related.as_deref(), Some("uomId"));

    let uom = model.entity("moqui.basic.Uom").unwrap();
    let reverse = uom.relationship("To#moqui.basic.UomConversion").unwrap();
    assert!(reverse.is_auto_reverse);
    assert_eq!(reverse.relationship_type, "many");
    assert_eq!(reverse.related, "moqui.basic.UomConversion");
    assert!(reverse.mutable);
    assert_eq!(reverse.key_map.len(), 1);
    assert_eq!(reverse.key_map[0].field_name, "uomId");
    assert_eq!(reverse.key_map[0].related.as_deref(), Some("toUomId"));
    assert!(uom.has_dependents);
    assert!(uom.relationship("toConversions").is_some());
}

fn id_conf() -> MNode {
    MNode::parse_text(
        "conf.xml",
        r#"<database-list>
            <dictionary-type type="id" java-type="java.lang.String" default-sql-type="VARCHAR(40)"/>
        </database-list>"#,
    )
    .unwrap()
}

#[test]
fn a_short_name_in_two_packages_means_the_framework_one_or_the_last_full_name() {
    let document = MNode::parse_text(
        "entities.xml",
        r#"<entities>
            <entity entity-name="Same" package="two.b" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
            </entity>
            <entity entity-name="Same" package="one.a" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
            </entity>
            <entity entity-name="Other" package="zz.c" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
            </entity>
            <entity entity-name="Other" package="moqui.c" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
            </entity>
        </entities>"#,
    )
    .unwrap();
    let model = EntityModel::load(&id_conf(), &[document]).unwrap();
    assert!(model.entity("one.a.Same").is_ok());
    assert!(model.entity("two.b.Same").is_ok());
    assert_eq!(model.entity("Same").unwrap().full_name, "two.b.Same");
    assert_eq!(model.entity("Other").unwrap().full_name, "moqui.c.Other");
    assert_eq!(model.full_names().len(), 4);
    assert!(model.entity("one.a.Same").unwrap().field("lastUpdatedStamp").is_none());
}

#[test]
fn an_entity_defined_again_overrides_and_an_unmatched_extend_is_skipped() {
    let first = MNode::parse_text(
        "first.xml",
        r#"<entities>
            <entity entity-name="Twice" package="one.a" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
                <field name="old" type="id"/>
            </entity>
        </entities>"#,
    )
    .unwrap();
    let second = MNode::parse_text(
        "second.xml",
        r#"<entities>
            <entity entity-name="Twice" package="one.a" no-update-stamp="true">
                <field name="id" type="id" is-pk="true"/>
                <field name="new" type="id"/>
            </entity>
            <extend-entity entity-name="Twice" package="other.pkg"><field name="extra" type="id"/></extend-entity>
            <extend-entity entity-name="Missing" package="one.a"><field name="extra" type="id"/></extend-entity>
        </entities>"#,
    )
    .unwrap();
    let model = EntityModel::load(&id_conf(), &[first, second]).unwrap();
    let twice = model.entity("one.a.Twice").unwrap();
    assert!(twice.field("new").is_some() && twice.field("old").is_none());
    assert!(twice.field("extra").is_none());
    assert_eq!(model.full_names().len(), 1);
}

#[test]
fn load_paths_reads_a_file_as_well_as_a_directory() {
    let dir = std::env::temp_dir().join(format!("moqui-model-paths-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("entity")).unwrap();
    std::fs::write(
        dir.join("entity/A.xml"),
        r#"<entities><entity entity-name="InDir" package="p" no-update-stamp="true"><field name="id" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    let file = dir.join("Extra.xml");
    std::fs::write(
        &file,
        r#"<entities><entity entity-name="InFile" package="p" no-update-stamp="true"><field name="id" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    let model = EntityModel::load_paths(&id_conf(), &[dir.join("entity").as_path(), file.as_path()]);
    let _ = std::fs::remove_dir_all(&dir);
    let model = model.unwrap();
    assert!(model.entity("p.InDir").is_ok());
    assert!(model.entity("p.InFile").is_ok());
}

/// A `pq-expression` alias is kept apart from the view's fields with its expression, as Java's `pqExpressionNodeMap`,
/// and its name keeps `alias-all` from aliasing a member field of the same name. `default-display` is kept on an alias.
#[test]
fn a_pq_expression_alias_is_kept_apart_and_default_display_is_kept() {
    let document = MNode::parse_text(
        "entities.xml",
        r#"<entities>
            <entity entity-name="Item" package="t" no-update-stamp="true">
                <field name="itemId" type="id" is-pk="true"/><field name="code" type="id"/>
                <field name="label" type="id"/></entity>
            <view-entity entity-name="ItemView" package="t">
                <member-entity entity-alias="I" entity-name="t.Item"/>
                <alias name="itemId" entity-alias="I"/>
                <alias name="code" entity-alias="I" default-display="false"/>
                <alias name="shown" pq-expression="code + '!'" default-display="true"/>
                <alias name="label" pq-expression="itemId + ':' + code"/>
                <alias-all entity-alias="I"/>
            </view-entity>
        </entities>"#,
    )
    .unwrap();
    let model = EntityModel::load(&id_conf(), &[document]).unwrap();
    let view = model.entity("t.ItemView").unwrap();
    assert_eq!(view.field_names(), ["itemId", "code"]);
    assert!(!view.has_field("shown") && !view.has_field("label"));
    assert_eq!(view.alias("code").unwrap().default_display, Some(false));
    assert_eq!(view.alias("itemId").unwrap().default_display, None);
    assert_eq!(
        view.pq_aliases,
        [
            PqAlias {
                name: "shown".to_string(),
                expression: "code + '!'".to_string(),
                default_display: true,
            },
            PqAlias {
                name: "label".to_string(),
                expression: "itemId + ':' + code".to_string(),
                default_display: false,
            },
        ]
    );
    assert_eq!(view.pq_alias("label").unwrap().expression, "itemId + ':' + code");
    assert!(view.pq_alias("code").is_none());
}
