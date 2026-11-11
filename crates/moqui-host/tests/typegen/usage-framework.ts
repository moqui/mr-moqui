// Authored TypeScript against the declarations `moqui types` writes for the framework and the tools component.
// `tsc --noEmit` passes only when every line without a marker type-checks and every `@ts-expect-error` line is an error.

export {};

// A framework entity: the field types are what the host's get returns.
const enumeration = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "ArtifactType").list().get(0);
const sequence: number | null | undefined = enumeration?.get("sequenceNum");
const enumTypeId: string | null | undefined = enumeration?.get("enumTypeId");
// @ts-expect-error not a field of moqui.basic.Enumeration
enumeration?.get("noSuchField");
const type = enumeration?.findRelatedOne("moqui.basic.EnumerationType");
const typeDescription: string | null | undefined = type?.get("description");
void [sequence, enumTypeId, typeDescription];

// A framework service: enumTypeId is required, the output is the declared list.
const found = ec.service.sync().name("org.moqui.impl.BasicServices.find#Enumeration").parameters({ enumTypeId: "ArtifactType" }).call();
const list: unknown[] | null | undefined = found.enumerationList;
// @ts-expect-error enumTypeId is required
ec.service.sync().name("org.moqui.impl.BasicServices.find#Enumeration").parameters({});
// An implicit entity-auto verb over a framework entity.
ec.service.sync().name("update#moqui.security.ArtifactGroup").parameters({ artifactGroupId: "G", description: "d" }).call();
// @ts-expect-error update needs the primary key
ec.service.sync().name("update#moqui.security.ArtifactGroup").parameters({ description: "d" });
void list;

// A tools screen: the keys its actions prove.
const detail = context as MoquiScreenContexts["component://tools/screen/System/Access/ArtifactGroupDetail.xml"];
const group: EntityValue<"moqui.security.ArtifactGroup"> | null | undefined = detail.artifactGroup;
const members: EntityList<"moqui.security.ArtifactGroupMember"> | undefined = detail.memberList;
void [group, members];

// A server-static tools shell: the props of its template.
declare const props: MoquiStaticScope<MoquiScreenPackages["component://tools/screen/System.xml"]>;
const locale: string = props.ec.user.locale;
// @ts-expect-error the browser ec has no service facade
props.ec.service;
void locale;
