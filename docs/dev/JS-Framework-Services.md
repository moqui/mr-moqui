# Framework services for application JavaScript

The framework ships services for the things most applications need: users and sign-in factors, email, chat, calls, files, wiki pages, search, reports, printing, notifications, system messages, git, and the tools that administer a server. Application JavaScript calls them through `ec.service`, and XML actions through `<service-call>`. This is a guide by group, to answer "is there already a service for this": what each group is for, its most useful services with their main parameters, and where the rest are. It is not a complete listing; [the complete list](#the-complete-list) says where that is.

In the tables a parameter marked `*` is required. Parameters and results are abbreviated to the main ones. A service's `description` in its XML file is the reference for its behaviour.

## Calling a service

```js
// synchronous: the results are a map
const out = ec.service.sync().name("org.moqui.impl.BasicServices.find#Enumeration")
    .parameters({ enumTypeId: "UserGroupType" }).call();
for (const row of out.enumerationList) { /* ... */ }

// asynchronous: on the worker pool, in its own transaction
ec.service.async().name("org.moqui.ui.NotificationServices.send#WebPush")
    .parameters({ userIdList: [userId], title: "Order shipped" }).call();
const future = ec.service.async().name("my.Services.build#Report").parameters({ reportId }).callFuture();
const result = future.get(30000);            // out-parameters; throws when not done in time

// a ServiceJob by name, now, with parameters over the job's own: the run id
const jobRunId = ec.service.job("clean_ServiceJobRun_daily").parameter("daysToKeep", 30).run();

// when the transaction in place commits (or rolls back)
ec.service.special().name("my.Services.notify#Shipped").parameters({ shipmentId }).registerOnCommit();
```

| Form | |
|---|---|
| `ec.service.sync()` | Runs in the caller's thread. `call()` returns the out-parameters. A failed call adds its errors to `ec.message` and the caller checks `ec.message.hasError()` ([Services](../logic/Services.md)) |
| `ec.service.async()` | `call()` queues the call and returns nothing; `callFuture()` returns an object with `isDone()` and `get(timeoutMillis)` |
| `ec.service.job(jobName)` | `parameters(map)`, `parameter(name, value)`, `run()`. A job that cannot start is an error in `ec.message` and `run()` returns null |
| `ec.service.special()` | `name(...)`, `parameters(map)`, `parameter(name, value)`, then `registerOnCommit()` or `registerOnRollback()` |
| `<service-call name="..." in-map="..." out-map="..."/>` | The same call from XML actions ([JS-XML-Actions](JS-XML-Actions.md)) |

The builder members (`disableAuthz()`, `requireNewTransaction()`, `softValidate()`, `transactionTimeout()`, ...) are in [JS-Core-API](JS-Core-API.md). Writing a service is in [JS-Service-Scripts](JS-Service-Scripts.md), and what a failed call does to the transaction is in [Services](../logic/Services.md).

### Service names

A service's name is the path of its file below a component's `service/` directory with dots, then `verb#noun`: `service/org/moqui/impl/UserServices.xml` with `<service verb="update" noun="Password">` is `org.moqui.impl.UserServices.update#Password`. A service with no noun is `path.verb` (`org.moqui.impl.BasicServices.noop`).

The authorization action of a call comes from the service's `authz-action`, else from its verb: `create` is create, `update` and `store` are update, `delete` is delete, `view`, `find`, `get` and `search` are view, and any other verb is all.

### Implicit entity-auto services

`create#`, `update#`, `store#` and `delete#` with an entity name are services with no definition. The entity name is any name the entity facade resolves.

```js
const made = ec.service.sync().name("create#moqui.basic.Enumeration")
    .parameters({ enumTypeId: "MyType", description: "First" }).call();   // made.enumId is sequenced
ec.service.sync().name("store#moqui.security.UserPreference")
    .parameters({ userId, preferenceKey: "my.key", preferenceValue: "on" }).call();
```

| Verb | |
|---|---|
| `create#Entity` | Creates the record. With one primary key field left out, it gets the entity's next sequenced id. With several, when the last one is left out it gets the next secondary sequence within the others (an item sequence id). A `fromDate` primary key left out is now |
| `update#Entity` | Updates the record of the primary key given. With a `statusId` given on an entity that has that field, the results add `oldStatusId` and `statusChanged`, and a changed status needs a `StatusFlowTransition` from the old one to the new one |
| `store#Entity` | Creates the record when any primary key field is missing (sequenced as `create#`), else creates or updates it by its primary key. The `statusId` check of `update#` applies when the record exists |
| `delete#Entity` | Deletes the record. Every primary key field is needed; `*` in one or more of them deletes each record that matches the other fields given |

All four share these rules:

- Only the entity's fields are read from the parameters; other names are ignored. An empty string sets a field to null.
- The results are the record's primary key fields, so a `create#` returns the id it made.
- A parameter named as a relationship of the entity, holding a map or a list of maps, stores those related records with the parent's key filled in (a `store#` of each; `delete#` deletes only the record named).
- `<field>_changeReason` beside a field goes to that field's audit log row.
- An implicit call needs no login and has no authorization of its own; each entity operation it makes is authorized as any other. It joins the transaction in place or begins one.
- The entities that only their own services write (the database audit and the like) are refused.

A service file can also declare an entity-auto service (`type="entity-auto"`, the noun an entity name) to give it parameters, authentication or ECA rules, as `org.moqui.impl.BasicServices.create#UomConversion` does.

### Interfaces

A service of `type="interface"` has parameters and no body; calling one is an error. Another service takes its parameters with `<implements service="..."/>`, and adds its own. The framework uses interfaces where it calls a service whose name is data: the `sendServiceName` of a `SystemMessageType`, the `feedReceiveServiceName` of a `DataFeed`, an Email ECA rule's action, a shell provider's hook.

```xml
<service verb="send" noun="OrderMessage">
    <implements service="org.moqui.impl.SystemMessageServices.send#SystemMessage"/>
    <actions>...</actions>
</service>
```

### Implementations

| Implementation | |
|---|---|
| Inline actions | `<actions>` in the service file: XML actions and `<script>` JavaScript. The mail client, chat, files, wiki and Comms services are written this way |
| A `.js` module | The actions run one function of a module beside the XML file: `ec.resource.script('component://framework/service/org/moqui/call/Calls.js').execute('start', context)`. The call, admin, public account and chat key services are written this way |
| `type="rust"` | The `location` is a Rust function path such as `moqui_git::service::get_refs`. The registry is `crates/moqui-core/src/service/rust_services.rs`; each module registers its functions with `rust_service!`, and the framework crates (`moqui-mail`, `moqui-git`, `moqui-tools`, `moqui-dav`, `moqui-collab`, `moqui-devkit`, `moqui-llm`) add theirs. A component's own are in [Native-Components](Native-Components.md) |
| `type="entity-auto"` | The entity-auto runner, with the declared parameters |
| `type="interface"` | No body |

A Rust service whose crate or feature is left out of a build still loads, so its definition, its types and its forms are there. A call fails with `service <name>: <feature> is not built into this host`. The features are listed by location prefix in `FEATURE_LOCATIONS` (`image`, `xlsx`) and in `crates/moqui-core/src/extension.rs` (`git`, `collab`, `tools`, `dav`, `devkit`, `smtp`, `imap`, `sip`, `test-client`, and `llm` for the `moqui_llm::` services). `ec.factory.hasFeature(name)` tells a script before it calls. The packages and their features are in [Build-Test](../Build-Test.md#packages).

### The complete list

- `moqui types` writes `services.d.ts`: every loaded service with its typed input and output map, so an editor completes names and parameters ([JS-Types-Tooling](JS-Types-Tooling.md)).
- The Tools app: Tools › Service lists the services, shows one service's parameters and runs it (`runtime/base-component/tools/screen/Tools/Service/`).
- `moqui autodoc` writes Markdown pages of the services, entities and screens ([Autodoc](Autodoc.md)).
- `moqui refs <service name>` lists what calls a service.
- The definitions: `runtime/base-component/framework/service/`, `runtime/base-component/basic-comms/service/` and `runtime/base-component/tools/service/`.

A file named `*.rest.xml` in a service directory (`tools/service/moqui.rest.xml`) is not a service file. It maps REST paths and methods to services for `/rest/s1`.

## Basics: `org.moqui.impl`

The services Moqui has always had under `org.moqui.impl`. Mostly inline actions; printing, images, entity sync and the user's keys are Rust.

### BasicServices

Enumerations, statuses, geos and units for drop-downs and checks.

| Service | |
|---|---|
| `find#Enumeration` | `enumTypeId*`, `orderByField` → `enumerationList` |
| `find#EnumerationByParent` | `parentEnumId*`, `includeParent`, `includeNested` → `enumerationList`, `enumIdSet` |
| `get#EnumsByTypeForDropDown` | `enumTypeId` → `resultList` |
| `find#StatusItem` | `statusTypeId*` → `statusItemList` |
| `find#StatusFlowTransitionToDetail` | `statusId*`, `statusFlowId` → `statusDetailList`: the statuses a record may go to |
| `get#GeoRegionsForDropDown` | `geoId`, `geoAssocTypeEnumId`, `geoTypeEnumId`, `term` → `resultList` |
| `convert#Uom` | `uomId*`, `toUomId*`, `amount*`, `effectiveDate` → `convertedAmount`, by the `UomConversion` records |
| `noop` | Nothing; a job or a transition that needs a service name |

Also `create#` and `update#UomConversion`, and `echo#Data` for tests.

### UserServices

Accounts, passwords, second factors and the current user's keys.

| Service | |
|---|---|
| `create#UserAccount` | `username*`, `newPassword*`, `newPasswordVerify*`, `emailAddress`, `requirePasswordChange`, the other `UserAccount` fields → `userId` |
| `update#UserAccount` | `userId*` and the `UserAccount` fields |
| `update#Password` | `oldPassword*`, `newPassword*`, `newPasswordVerify*`, `userId` or `username`, `code` → `updateSuccessful`, `passwordIssues` |
| `update#PasswordInternal` | `userId*`, `newPassword*`, `newPasswordVerify*`, `requirePasswordChange` → `updateSuccessful`, `passwordIssues`. No old password: for server code only |
| `reset#Password` | `userId` or `username` (or the email address as `username`): emails a reset password. Anonymous, and answers the same whether or not the account exists |
| `enable#UserAccount`, `disable#UserAccount` | `userId*` |
| `set#Preference` | `preferenceKey*`, `preferenceValue`: the current user's preference |
| `end#UserVisit` | `visitId*` → `sessionCount`: end another session of the current user |
| `get#UserAuthcFactorInfo` | `userId*` → `secondFactorRequired`, `factorTypeEnumIds`, `userAuthcFactorList`, `sendableFactors` |
| `create#UserAuthcFactorEmail`, `create#UserAuthcFactorSms` | `userId*`, `factorOption*` (the address or number) → `factorId` |
| `create#UserAuthcFactorTotp`, `setup#UserAuthcFactorTotp` | `userId*` → `factorId`, `factorOption`; the QR values `qrUri`, `dataUri` |
| `validate#UserAuthcFactorCode` | `userId*`, `code*` → `verified`, `factorId` |
| `create#SingleUseAuthcCodes` | `userId*`, `numberOfCodes` → `singleUseCodes` |
| `create#UserAccessToken`, `revoke#UserAccessToken` | `description`, `expireDays` → `accessToken`, `loginKey`, `thruDate`. The token is shown once |
| `create#UserAppPassword`, `revoke#UserAppPassword`, `get#UserAppPasswords` | `name*` → `appPasswordId`, `password`: for a device on DAV or SFTP |
| `create#UserSshKey`, `delete#UserSshKey` | `publicKey*`, `description` → `sshKeyId`, `fingerprint` |
| `get#UserDeviceKeys`, `revoke#UserDeviceKey`, `renew#UserDeviceKey` | The devices signed in by key (the desktop and mobile clients) |

The rest are the login steps (`login#UserAccount`, `validate#ExternalUserAuthcCode`, `send#ExternalAuthcCode`, `send#AuthcCodeEmail`, `send#AuthcCodeSms`) and `create#InitialAdminAccount`.

### SystemMessageServices

Messages to and from other systems, kept as `SystemMessage` records with a status, retried by jobs.

| Service | |
|---|---|
| `queue#SystemMessage` | `systemMessageTypeId*`, `messageText*`, `systemMessageRemoteId`, `sendNow` → `systemMessageId`. An outgoing message, sent now by default |
| `receive#IncomingSystemMessage` | `systemMessageTypeId*`, `messageText*`, `systemMessageRemoteId`, `remoteMessageId` → `systemMessageIdList`, `duplicateOfId` |
| `store#ReceivedSystemMessage` | `systemMessageTypeId*`, `systemMessageRemoteId*`, `messageId*`, `messageText*` → `systemMessageId`, `duplicateOfId`: for a provider's own receive service |
| `send#ProducedSystemMessage`, `consume#ReceivedSystemMessage` | `systemMessageId*`: send or consume one now |
| `send#AllProducedSystemMessages`, `consume#AllReceivedSystemMessages` | `retryMinutes`, `retryLimit`: the jobs |
| `cancel#SystemMessage`, `reset#SystemMessageInError` | `systemMessageId*` |
| `send#SystemMessageRest`, `send#SystemMessageDirectLocal` | Send services to name in `SystemMessageType.sendServiceName` |

Interfaces to implement: `send#SystemMessage` (`systemMessageId*` → `remoteMessageId`), `consume#SystemMessage` (`systemMessageId*`), `receive#SystemMessage`, `produce#AckSystemMessage`. `org.moqui.sftp.SftpMessageServices` has `send#SystemMessageSftp` and `poll#SystemMessageSftp` (`systemMessageTypeId*`, `systemMessageRemoteId*`, `fileCountLimit` → `fileCount`, `failedFileList`) for messages as files on an SFTP server.

### EntityServices, EntitySyncServices

Data documents and feeds, entity snapshots, and sync between servers.

| Service | |
|---|---|
| `get#DataFeedDocuments` | `dataFeedId*`, `fromUpdateStamp`, `thruUpdateStamp` → `documentList` |
| `get#DataFeedLatestDocuments` | `dataFeedId*` → `documentList`, and moves the feed's `lastFeedStamp` |
| `create#DataDocument`, `clone#DataDocument`, `create#DataDocumentField` | Defining data documents in the database |
| `export#EntityDataSnapshot` | `fromDate`, `thruDate`, `entitiesToInclude`, `entitiesToSkip`, `baseFilename`, `fileType` → `recordsWritten` |
| `import#EntityDataSnapshot` | `zipFilename*`, `dummyFks`, `useTryInsert`, `disableEntityEca`, ... → `recordsLoaded` |
| `rewrite#EntityEncryptedFields` | `entityName*` → `recordsUpdated`: after a change of encryption key |
| `EntitySyncServices.run#EntitySync` | `entitySyncId*`: one sync of an `EntitySync` record, push or pull |

The top-level `org.moqui.EntityServices` has the interfaces a feed or a document names: `receive#DataFeed` (`documentList*`, `dataFeedId`, `feedStamp`), `receive#DataFeedDelete`, `add#ManualDocumentData`, `transform#DocumentMapping`; and `process#DataFeedPending`, the job that delivers pending feed rows.

### ScreenServices

Form responses, database forms and scheduled screen renders.

| Service | |
|---|---|
| `create#FormResponse` | `formLocation*`, `formId`, `responseMap`, `userId` → `formResponseId` |
| `get#FormResponse` | `formResponseId*` → the `FormResponse` fields, `responseMap`, `dbForm`, `dbFormFieldList` |
| `update#FormResponse`, `delete#FormResponse` | `formResponseId*`, `responseMap*` |
| `get#FormResponseStats` | `formId*` → `formResponseCount` and counts by field |
| `publish#DbForm` | `formId*`, `fromDate`, `fieldNames` → `publishedFieldNames`: start the drafts of a dated form |
| `evaluate#DbFormDependencies` | `formId*`, `values`, `asOfDate` → `visibleFieldNames`, `hiddenFieldNames`, `requiredFieldNames`, `missingFieldNames` |
| `render#ScheduledScreens` | The job that renders and emails each `ScreenScheduled` |

The other DbForm services (`update#DbFormField`, `end#DbFormField`, `create#DbFormFieldSuccessor`, `store#DbFormFieldDependency`, ...) are what the form editor calls ([Db-Forms](../ui/Db-Forms.md)).

### PrintServices, ImageServices

| Service | |
|---|---|
| `PrintServices.print#Document` | `networkPrinterId*`, `inputStream` or `serialBlob`, `contentType`, `jobName`, `copies`, `duplex`, `pageRanges` → `printJobId`, `jobId` |
| `PrintServices.print#ResourceDocument` | `networkPrinterId*`, `resourceLocation*` → `printJobId`, `jobId` |
| `PrintServices.print#ScreenDocument` | `networkPrinterId*`, `screenLocation*`, `screenParameters`, `contentType` → `printJobId`, `jobId`: the screen as a PDF |
| `PrintServices.get#ServerPrinters` | `serverHost*`, `serverPort` → `networkPrinterIdList`: a `NetworkPrinter` for each printer of an IPP server |
| `PrintServices.hold#PrintJob`, `release#PrintJob`, `cancel#PrintJob`, `get#PrintJobDetailsFromServer` | `printJobId` |
| `ImageServices.convert#Image` | `inputLocation` or `inputImageStream`, `outputImageFormat*`, `outputResourceReferenceLarge` (and `Original`, `Medium`, `Small`, `Thumbnail`), `pixelWidthLarge*` (and the others) → `contentLocationLarge`, ... |
| `ImageServices.write#Image` | `image*` (bytes or a location), `outputResourceReference*`, `outputImageFormat*` → `contentLocation`, `succeeded` |

The image services need the `image` feature.

### ServerServices, ServiceServices

Housekeeping jobs: `ServerServices.clean#PrintJobData` and `ServiceServices.clean#ServiceJobRun` (`daysToKeep`, `maxSeconds`), `ServerServices.backup#EmbeddedDatabase` (`dailyCount`, `weeklyCount`, `monthlyCount`, `backupDir`, `systemMessageRemoteId` → `archivePath`, `backupKind`, `bytesWritten`), and `ServerServices.get#VisitClientIpData` (`visitId*`).

## Top-level `org.moqui`

The files directly under `org/moqui` hold the interfaces that have kept their Java names, and one service.

| Service | |
|---|---|
| `EmailServices.send#EmailTemplate` | Interface: the parameters of sending a template ([Mail](#mail-orgmoquimail)) |
| `EmailServices.process#EmailEca` | Interface: what an Email ECA rule's service receives (`emailServerId`, `message`, `fields`, `bodyPartList`, `headers`, `flags`) |
| `GitServices.process#GitEca` | Interface: what a gitECA rule's service receives (`repositoryId`, `refName`, `oldId`, `newId`, `newCommits`, ...) |
| `ImageServices.convert#Image` | Interface |
| `SmsServices.send#SmsMessage` | `contactNumber*`, `message*`, `countryCode`, `areaCode`, `fromNumber`, `userId` → `successful`, `messageId`, `errorMessage`, `reasonCode`. Sends a text through the messaging bridge of the system's number |

## Mail: `org.moqui.mail`

`EmailServices` sends and receives; `EmailClientServices` is the mail client's model (accounts, folders, threads, drafts, rules) that the Comms Inbox is built on. Sending and the server sync are Rust (`moqui-mail`, the `smtp` and `imap` features); the client services are inline actions. See [Mail-Inbound](../interfaces/Mail-Inbound.md) and [Comms](../tools/Comms.md).

| Service | |
|---|---|
| `EmailServices.send#EmailTemplate` | `emailTemplateId*`, `toAddresses*`, `ccAddresses`, `bccAddresses`, `bodyParameters`, `attachments`, `createEmailMessage`, `fromUserId`, `toUserId`, `inReplyTo` → `messageId`, `emailMessageId`, `deliveryOutcome`. For internal use: no login is required, so do not expose it |
| `EmailServices.send#Email` | The same with `body` and `bodyText` given, and a login required |
| `EmailServices.send#EmailMessage` | `emailMessageId*`, `markSent` → `messageId`, `sendError`, `deliveryOutcome`: send a stored `EmailMessage` |
| `EmailServices.send#ScreenRenderEmail` | `screenLocation*`, `screenRenderMode*`, `toAddresses*`, `fileName`, `emailTemplateId`, `bodyParameters`: a rendered screen as an attachment |
| `EmailServices.poll#EmailServer` | `emailServerId*`: receive from IMAP or POP3 and run the Email ECA rules |
| `EmailServices.test#EmailServer` | `emailServerId*`, `testSmtp`, `testStore` → `success`, `stepList`, `folderList` |
| `EmailClientServices.get#UserEmailAccounts` | → `accountList`, `internalEmailAccountId` |
| `EmailClientServices.create#EmailAccount` | `emailAddress*`, `accountName`, `smtpHost`, `storeHost`, `mailUsername`, `mailPassword`, ... → `emailAccountId`, `emailServerId`, `emailIdentityId` |
| `EmailClientServices.sync#EmailAccount` | `emailAccountId*`, `inboxOnly`, `pushOnly` → `outcome`, `newCount`, `updatedCount`, `removedCount` |
| `EmailClientServices.find#Messages` | `emailAccountId`, `view`, `emailFolderId`, `emailLabelId`, `searchText`, `unreadOnly`, `threadMode`, `beforeDate`, `pageSize` → `threadList`, `hasMore`, `nextBeforeDate` |
| `EmailClientServices.get#Thread` | `emailMessageId` or `rootEmailMessageId` → `messageList`, `subject`, `canSend`, `unreadCount` |
| `EmailClientServices.save#Draft` | `emailMessageId`, `emailAccountId` or `emailIdentityId`, `toAddresses`, `ccAddresses`, `subject`, `body`, `attachmentList`, `parentEmailMessageId` → `emailMessageId`, `draftStamp`, `conflict` |
| `EmailClientServices.send#Draft` | `emailMessageId*`, `scheduledSendDate`, `holdSeconds` → `statusId`, `sent`, `messageId`, `sendError` |
| `EmailClientServices.set#MessageFlags` | `emailMessageId`, `emailMessageIdList` or `rootEmailMessageId`, `read`, `flagged`, `answered` → `updatedCount` |
| `EmailClientServices.move#Messages`, `apply#Labels`, `trash#Messages`, `archive#Messages`, `snooze#Messages` | The same message selection, with `emailFolderId*`, `addLabelIdList` and `removeLabelIdList`, `deleteForever` |
| `EmailClientServices.count#UnreadEmail` | `userId` → `unreadCount`, `unreadThreadCount`, `accountCountMap` |
| `EmailClientServices.import#EmailMessages` | `emailFolderId*`, `file`, `fileList`, `location` or `messageText` → `importedCount`, `duplicateCount`, `failedCount` (`.eml` and `.mbox`) |
| `EmailClientServices.run#EmailRules` | `emailAccountId`, `emailFolderId`, `emailRuleId`, `dryRun` → `matchedCount`, `actionCount`, `matchList` |

`EmailClientServices` has about ninety services: folders and labels, identities, rules, out of office, text blocks, saved searches, OAuth sign-in of accounts ([Mail-OAuth](../interfaces/Mail-OAuth.md)), scheduled send and snooze jobs, and the change log the clients sync from. Read `runtime/base-component/framework/service/org/moqui/mail/EmailClientServices.xml`.

## Chat: `org.moqui.chat`

Rooms, members, messages and presence in `ChatServices`; end-to-end encryption keys in `ChatKeyServices` ([Chat-Keys](../interfaces/Chat-Keys.md)); bridges to other networks in `ChatBridgeServices` ([Chat-Bridges](../interfaces/Chat-Bridges.md)); what the XMPP server asks in `XmppServices` ([XMPP](../interfaces/XMPP.md)). Mostly inline actions. Many refusals are answers, not errors: read `reasonCode`.

| Service | |
|---|---|
| `ChatServices.create#Room` | `roomTypeEnumId`, `roomName`, `topic`, `memberUserIdList`, `recordEntityName`, `recordId`, `encrypted` → `chatRoomId`, `created`, `reasonCode`. A direct room is one per set of users |
| `ChatServices.find#Rooms` | `filter`, `searchText`, `includeArchived`, `recordEntityName`, `recordId`, `limit` → `roomList`, `unreadTotal`, `mentionTotal` |
| `ChatServices.get#Room` | `chatRoomId`, or `recordEntityName` and `recordId` → `room`, `member`, `access`, `memberList` |
| `ChatServices.add#Member`, `remove#Member` | `chatRoomId*`, `userId` or `userIdList`, `roleEnumId` → `addedUserIdList`. With no user, the current user joins or leaves |
| `ChatServices.send#Message` | `chatRoomId*`, `body` (safe HTML), `replyToEventSeq`, `threadRootEventSeq`, `chatAttachmentIdList`, `mentionUserIdList`, `clientRef` → `eventSeq`, `event`, `duplicate`, `reasonCode` |
| `ChatServices.edit#Message`, `redact#Message`, `react#Message`, `pin#Message` | `chatRoomId*`, `eventSeq*`, then `body` or `key` |
| `ChatServices.get#Timeline` | `chatRoomId` (or the record), `beforeSeq`, `afterSeq`, `aroundSeq`, `limit`, `threadRootEventSeq` → `eventList`, `hasMoreBefore`, `hasMoreAfter`, `lastReadEventSeq` |
| `ChatServices.mark#Read` | `chatRoomId*`, `eventSeq`, `markUnread` → `lastReadEventSeq` |
| `ChatServices.get#UnreadCounts` | `userId` → `total`, `unread`, `mentions`, `roomCounts` |
| `ChatServices.upload#Attachment` | `chatRoomId*`, `file` or `uploadId` → `chatAttachmentId`, `location`, `fileName` |
| `ChatServices.search#ChatEvents` | `searchText*`, `chatRoomId`, `limit`, `pageIndex` → `eventList`, `total` |
| `ChatServices.create#Poll`, `vote#Poll`, `close#Poll` | `chatRoomId*`, `question*`, `optionList*`, `anonymous`, `multiple` → `eventSeq` |
| `ChatServices.set#UserStatus`, `get#UserStatuses` | `status` (online, away, dnd, invisible), `statusText`, `emoji`, `clearAfterMinutes` → `myStatus` |

The rest of `ChatServices` is link previews, retention jobs, room and user change logs for clients, and the internal services of delivery. The Comms screens call the wrappers in [basic-comms](#comms-screens-basic-comms).

## Calls: `org.moqui.call`

Audio and video calls between users (`CallServices`), phone calls through SIP trunks (`VoiceServices`), and what surrounds them: transfers, queues, call handling rules, IVR menus, recording, captions, voicemail, SMS rooms, SIP devices and speech to text. Inline actions over the `.js` modules beside the files (`Calls.js`, `Voice.js`, `Voicemail.js`, ...); the media and SIP engines are Rust. Refusals are a `reasonCode`, and a repeated call answers `repeated`. See [WebRTC](../interfaces/WebRTC.md), [SIP](../interfaces/SIP.md), [Voicemail](../interfaces/Voicemail.md), [Call-Recording](../interfaces/Call-Recording.md) and [SMS](../interfaces/SMS.md).

| Service | |
|---|---|
| `CallServices.start#Call` | `calleeUserIdList` or `chatRoomId`, `video`, `idempotencyKey`, `requireE2ee` → `callSessionId`, `callLegId`, `call`, `reasonCode` |
| `CallServices.answer#Call`, `join#Call`, `decline#Call`, `leave#Call`, `end#Call` | `callSessionId*` → `callLegId`, `call`, `reasonCode` |
| `CallServices.get#Call` | `callSessionId*` → `call`: its state, participants and the user's own leg |
| `CallServices.find#Calls` | `outcome`, `pageIndex`, `pageSize` → `callList`, `callListCount`: the user's call history |
| `CallServices.count#MissedCalls`, `mark#MissedSeen` | `userId` → `missedCount` |
| `CallServices.create#CallLink` | `name*`, `chatRoomId`, `policy`, `video`, `expiryHours` → `callLinkId`, `token`, `path`: a meeting link for guests |
| `CallServices.get#IceServers` | `callSessionId*` → `iceServers` ([TURN](../interfaces/TURN.md)) |
| `VoiceServices.place#PhoneCall` | `phoneNumber*`, `fromNumber`, `idempotencyKey`, `hideCallerId` → `callSessionId`, `callLegId`, `phoneLegId`, `call`, `reasonCode` |
| `VoiceServices.send#PhoneDtmf`, `hold#PhoneCall`, `resume#PhoneCall` | `callSessionId*`, `digits*` |
| `VoiceServices.get#VoiceCapabilities` | → `supported`, `configured`, `active`, `reasonCode` |
| `VoiceServices.find#MyCallerIds` | → `numberList` |
| `CallTransferServices.transfer#Call` | `callSessionId*`, `targetType*` (user, number or queue), `targetId`, `mode` → `callTransferId`, `call` |
| `CallTransferServices.add#CallParticipant` | `callSessionId*`, `userId` or `phoneNumber` → `callLegId` |
| `CallRecordingServices.start#Recording`, `stop#Recording`, `find#Recordings` | `callSessionId*`, `kind` → `callRecordingId`, `recording` |
| `VoicemailServices.find#Voicemails`, `get#Voicemail`, `mark#VoicemailHeard`, `count#UnheardVoicemails` | `folder`, `heard`, `pageIndex`; `voicemailId*` → `voicemailList`, `voicemail`, `unheardCount` |
| `SmsServices.open#PairRoom` | `toNumber*`, `fromNumber` → `chatRoomId`: the chat room that texts a number |
| `SpeechServices.transcribe#Audio` | `audioLocation` or `audio`, `mediaType*`, `language`, `prompt` → `text`, `reasonCode`, `errorText` |
| `CallHandlingServices.normalize#PhoneNumber` | `phoneNumber` → `number` (E.164), `destinationClass` |

The administration services (`find#`, `store#` and `delete#` of `VoiceTrunk`, `PhoneNumber`, `RingGroup`, `CallQueue`, `IvrMenu`, `CommsSchedule`, `CallerList`, `SipDevice`) and the internal ones of the media nodes are in the same files.

## Resources: `org.moqui.resource`

Files in drives (`FileServices`), wiki pages (`WikiServices`), spreadsheets, rich-text images, co-edited documents, and the DAV server's address. `FileServices` and `WikiServices` are inline actions; the rest are Rust. A git-backed drive or wiki space keeps versions as commits ([Revision-Management](../data/Revision-Management.md)).

### FileServices

A drive is a tree of files with members and permissions; every user has a personal drive.

| Service | |
|---|---|
| `get#UserDrives` | `includeArchived`, `includeUsage` → `driveList`, `personalDriveId` |
| `create#Drive` | `driveName*`, `description`, `storage` (folder, gitDb, gitLocal, ...), `quotaBytes`, `memberList` → `driveId`, `rootLocation` |
| `list#Folder` | `driveId*`, `path` → `fileList`, `folder`, `drive`, `breadcrumbList` |
| `get#ResourceInfo` | `driveId*`, `path*`, `versionName` → `resource`, with the location to read it from |
| `find#Resources` | `text`, `driveId`, `path`, `fileType`, `modifiedFrom`, `tagId`, `limit` → `resultList` |
| `upload#Resource` | `driveId*`, `path`, `file` or `fileList`, `versionNote`, `conflictPolicy` → `uploadedList`, `versionName` |
| `save#Text` | `driveId*`, `path*`, `text`, `versionNote` → `versionName` |
| `create#Folder` | `driveId*`, `path`, `name*` → `newPath` |
| `rename#Resource` | `driveId*`, `path*`, `newName*` → `newPath` |
| `move#Resource`, `copy#Resource` | `driveId*`, `path` or `pathList`, `toDriveId`, `toPath`, `conflictPolicy` → `movedList`, `copiedList` |
| `trash#Resource`, `restore#Resource`, `empty#Trash` | `driveId*`, `path` or `pathList` → `trashList` |
| `get#Versions`, `get#VersionDiff`, `restore#Version` | `driveId*`, `path*` → `versionList` |
| `share#Resource` | `driveId*`, `path`, `sharedWithUserId` (or a group, party, email, or `isLink`), `permissionEnumId`, `expireDate`, `password` → `shareId`, `linkToken`, `linkPath` |
| `download#Zip` | `driveId*`, `path` or `pathList` → `zipLocation`, `zipFilename`, `fileCount` |

Also members and ownership, favorites, tags, activity, previews, public share links (`open#PublicShare`, `list#PublicShare`, ...) and the index jobs.

### WikiServices

| Service | |
|---|---|
| `get#PublishedWikiPageText` | `wikiSpaceId*`, `pagePath`, `versionName`, `getPageText` → `wikiPageId`, `pageText`, `pageLocation`, `publishedVersionName` |
| `get#WikiPageInfo` | The parameters of `get#WikiPageReference`, `versionName` → `pageLocation`, `pageName`, `attachmentList`, `breadcrumbMapList`, `currentVersionName` |
| `update#WikiPage` | `wikiSpaceId*`, `pageName*`, `wikiPageId` or `pagePath`, `parentPath`, `pageText`, `wikiType`, `versionNote` → `wikiPageId`, `pagePath`, `versionName`, `createdRecord`. Creates the page when it is not there |
| `create#WikiSpace` | `wikiSpaceId*`, `rootPageDirectory`, `wikiType`, `repositoryKind`, `memberList` → `wikiSpaceId`, `rootPageLocation` |
| `move#WikiPage`, `delete#WikiPage` | `wikiPageId`, or `wikiSpaceId` and `pagePath`; `toWikiSpaceId`, `toParentPath`, `toPageName` |
| `get#WikiSpacePages` | `wikiSpaceId*`, `currentPagePath` → `allChildFileFlatList`, `rootChildResourceList` |
| `search#WikiPages` | `text*`, `wikiSpaceId`, `limit` → `pageList` |
| `get#WikiPageHistory`, `get#WikiPageVersionDiff`, `restore#WikiPageVersion` | The page → `versionList`, `currentVersionName`, `publishedVersionName` |
| `get#WikiPageBacklinks`, `get#WikiPageLinks` | The page → `backlinkList` |
| `upload#WikiPageAttachment` | The page, `attachmentFile` → `attachmentReference` |
| `request#WikiPageReview` | The page, `versionName`, `reviewerUserIdList`, `dueDate`, `note` → `wikiPageReviewIdList`; then `approve#`, `refuse#`, `cancel#WikiPageReview` |

### The others

| Service | |
|---|---|
| `SpreadsheetServices.read#Spreadsheet` | `location` or `contentBytes`, `sheetName` or `sheetIndex`, `headerRow`, `maxRows` → `sheetNameList`, `headerList`, `rowList`. The `xlsx` feature |
| `SpreadsheetServices.load#SpreadsheetEntity` | `entityName*`, `location` or `contentBytes`, `fieldMap`, `loadMode` → `recordCount` |
| `ResourceServices.upload#RichTextImage`, `get#RichTextImage` | `contentFile*` → `url`; `imageId*` → `location` |
| `ResourceServices.gc#ContentBlobs` | `dryRun` → `deletedCount`, `deletedBytes`: the job that removes unreferenced blobs |
| `CollabServices.get#CollabDocument`, `save#CollabDocument` | `documentKey*`, `versionNote` → `snapshotText`, `isOpen`, `versionName`. The `collab` feature |
| `DavServices.get#SyncAddresses` | `driveId` → `enabled`, `davUrl`, `filesUrl`, `calendarsUrl`, `addressBooksUrl`, `sftpUrl` ([Sync](../interfaces/Sync.md)). The `dav` feature |

## Search: `org.moqui.search`

`SearchServices` indexes and searches data documents through the search facade. Inline actions.

| Service | |
|---|---|
| `search#DataDocuments` | `indexName*`, `queryString*`, `documentType`, `orderByFields`, `highlightFields`, `pageIndex`, `pageSize`, `flattenDocument` → `documentList`, `documentListCount`, and the page values |
| `index#DataFeedDocuments` | `dataFeedId*`, `dataDocumentId`, `fromUpdateStamp`, `thruUpdateStamp`, `batchSize` → `documentsIndexed` |
| `index#DataDocuments` | Implements `receive#DataFeed`: the feed's receive service that indexes `documentList` |
| `delete#DataDocument` | `documentId*`, `indexName`, `dataDocumentId` |
| `delete#Documents` | `indexName*`, `timestampField`, `daysToKeep`, `queryString` → `deletedCount` |
| `index#WikiSpacePages` | `dataDocumentId*`, `wikiSpaceId` |
| `embed#PendingDocuments` | `indexName`, `documentType`, `batch` → `embedded`: for semantic search, when it is enabled |

`ElasticSearchServices.xml` only includes `delete#Documents` under its old name, for `ServiceJob` records that still name it.

## Reports and monitoring: `org.moqui.report`, `org.moqui.monitor`

`ReportServices` is the report engine: datasets, queries, saved definitions (dashboards, reports, alert rules), schedules and their deliveries. `MonitorServices` and `TelemetryServices` are the monitor's incidents, releases, recovery, runtime settings and telemetry jobs. All Rust. See [Monitoring](../tools/Monitoring.md).

| Service | |
|---|---|
| `ReportServices.list#Datasets`, `get#Dataset` | → `datasets`; `datasetId*` → `dataset` |
| `ReportServices.query#Documents` | `datasetId*`, `queryString`, `filters`, `from`, `thru`, `sort`, `pageIndex`, `pageSize` → `hits`, `total`, `nextCursor`, `notices` |
| `ReportServices.run#Query` | `query*` → `frames`, `range`, `notices` |
| `ReportServices.list#Definitions`, `get#Definition`, `save#Definition` | `definitionType`; `definitionId*`; `definition*`, `note` → `definition` |
| `ReportServices.render#Dashboard` | `definitionId*`, `variables`, `filters`, `from`, `thru`, `panelId` → `dashboard` |
| `ReportServices.preview#Report` | `definitionId*`, `renderMode`, `from`, `thru`, `variables` → `contentBase64`, `contentType`, `filename` |
| `ReportServices.create#Schedule`, `send#Report` | `definitionId*`, `cronExpression*`, `renderMode`, `userId` or `userGroupId` → `screenScheduledId` |
| `ReportServices.create#Annotation` | `text*`, `time`, `source`, `tags`, `serviceName`, `recordType`, `recordId`, `link` → `annotationId`: a change marker or a business event on the time series |
| `MonitorServices.create#Incident`, `update#Incident`, `add#IncidentNote`, `list#Incidents` | `title*`, `severity`, `serviceName`, `summary` → `incidentId`, `incident` |
| `MonitorServices.list#RuntimeSettings`, `set#RuntimeSetting` | `key`, `value*`, `minutes*`, `reason*`: a setting changed for a while on every instance |
| `TelemetryServices.clean#TelemetryData`, `rollup#Metrics`, `scrape#Targets`, `run#Checks` | The telemetry jobs |

`ReportServices` also has business datasets (`create#BusinessDataset`, ...), definition versions, export and import, git sync of definitions, and the owned-resource services of schedules and alert rules.

## User interface: `org.moqui.ui`

What the shell and shared widgets call. `NotificationServices` is inline actions with Rust for push, `StatusServices` is inline, `ShellServices` is Rust, and `UnitServices` runs `UnitPolicy.js`.

| Service | |
|---|---|
| `NotificationServices.get#UserNotifications` | `inboxEnumId`, `topic`, `viewed`, `fromDate`, `text`, `pageIndex`, `pageSize` → `notificationList`, `notificationListCount`, `unreadCounts`, `approvalCount` |
| `NotificationServices.mark#NotificationsViewed` | `notificationMessageId` or `notificationMessageIdList`, `inboxEnumId` → `viewedCount` |
| `NotificationServices.approve#NotificationMessage`, `refuse#NotificationMessage` | `notificationMessageId*`, `reason` → `results`: calls the topic's approve or refuse service |
| `NotificationServices.store#WebPushSubscription`, `delete#WebPushSubscription`, `get#WebPushStatus` | `endpoint*`, `p256dhKey*`, `authSecret*` → `subscriptionId` |
| `NotificationServices.register#PushDevice`, `unregister#PushDevice` | `provider*`, `token*`, `appId`, `platform`: a native app's FCM token |
| `NotificationServices.send#WebPush` | `userIdList*`, `title`, `link`, `topic`, `tag` → `sentCount`, `failedCount`. Internal; a persisted notification sends it |
| `ShellServices.get#ShellCounts`, `publish#ShellCounts` | `userId` → `counts`; `userIdList*`: the badge counts of the shell |
| `ShellServices.get#Providers`, `call#Providers` | `hook*`, `parameters`, `sourceId` → `providerList`; `results`, `failedParts`: the shell providers the apps declare |
| `ShellServices.get#Appearance`, `set#UserScreenTheme` | `screenThemeId` → `appearance` ([Themes](../ui/Themes.md)) |
| `StatusServices.get#StatusBar` | `statusId`, `statusTypeId`, `statusFlowId` → `current`, `stages`, `transitions`, `next` |
| `StatusServices.get#StatusColors` | `statusIdList` → `colors` |
| `UnitServices.get#UnitContext`, `store#Unit`, `store#UnitConversion`, `preview#UnitConversion`, `end#UnitConversion` | Units and conversions with revisions: `uomId`, `toUomId`, `conversionFactor`, `amount` → `unitRows`, `conversionRows`, `convertedAmount` |

A notification itself is sent with `ec.makeNotificationMessage()` ([JS-Core-API](JS-Core-API.md)), not a service.

## Sign-in: `org.moqui.sso`, `org.moqui.public`, `org.moqui.ext`

`AuthServices` is single sign-on through an `AuthFlow` (an identity provider) and the app login of the desktop and mobile clients; `RecentAuthServices` asks a user to confirm who they are before a sensitive step ([Recent-Auth](../security/Recent-Auth.md)). Both Rust. `public.AccountServices` is the account flow of a public site, each service taking one `input` map and answering one `result` map from `Account.js`, with `allow-remote="false"`.

| Service | |
|---|---|
| `sso.AuthServices.get#AppProviders` | → `authFlowList`: the identity providers a login page shows |
| `sso.AuthServices.login#User` | `authFlowId*`, `returnTo`, `reauth`: redirects to the identity provider |
| `sso.AuthServices.logout#User` | `returnTo` |
| `sso.AuthServices.check#Flow` | `authFlowId*` → `passed`, `message`, `callbackUrl` |
| `sso.RecentAuthServices.check#RecentAuth` | `maxAgeSeconds`, `required` → `recent`, `authAgeSeconds`, `method`, `secondFactor` |
| `sso.RecentAuthServices.reauthenticate#User` | `password`, `code` → `reauthenticated`, `codeNeeded`, `sendableFactors` |
| `public.AccountServices.start#Session`, `complete#Session`, `end#Session` | `input` → `result`: sign in and out |
| `public.AccountServices.begin#Registration`, `verify#Registration`, `request#Recovery`, `manage#Account` | `input` → `result` |
| `ext.GoogleServices.verify#ReCaptcha` | `reCaptchaToken`, `reCaptchaSecret`: an error when the token does not verify |

## Administration: `org.moqui.admin`

What the Admin app's screens call: access (groups, memberships, grants), configuration (instance settings, translations, identity providers, connections, protocols), identity (sessions and credentials of an account), operations (jobs, system messages, mail outcomes, notification topics) and the AI Constitution. Inline actions over `.js` modules (`Access.js`, `ConfigurationSettings.js`, `OperationJobs.js`, ...); the Constitution services are Rust (`llm`). The write services take the `expectedRevision` that the read gave and answer `applied`, `reasonCode` and `reason` instead of failing.

| Service | |
|---|---|
| `AccessServices.find#Groups`, `get#Group` | `text`, `appName`, `kind`; `userGroupId*` → `groupList`; `group`, `members`, `grants` |
| `AccessServices.store#Membership`, `end#Membership` | `userGroupId*`, `userId*`, `fromDate`, `thruDate`, `expectedRevision`, `reason` → `applied`, `revision` |
| `AccessServices.preview#Grants`, `apply#Grants` | `userGroupId*`, `grantList*`, `expectedRevision*` → `changeList`, `applied` |
| `ConfigurationServices.get#InstancePreference` | `settingKey*` → `value`, `numberValue`, `source`: a registered instance setting, for the code that uses it |
| `ConfigurationServices.get#EffectiveSettings`, `store#InstancePreference` | `settingKey`, `settingGroup`; `settingKey*`, `value`, `expectedRevision` |
| `ConfigurationServices.store#LocalizedMessage` | `original*`, `locale*`, `localized`, `expectedRevision` |
| `ConfigurationServices.find#Connections`, `store#Connection`, `test#Connection` | `adapterKey*`, `connectionKey`, `values`, `enabled` → `connectionList`; `passed`, `checkList` |
| `IdentityServices.find#Sessions`, `revoke#Sessions` | `userId`, `activeOnly`; `sessionKeyList`, `allOthers` → `sessionList`; `sessionCount` |
| `IdentityServices.get#AccountCredentials`, `revoke#AccountAccess` | `userId*`, `kindList`, `reason` → `factorList`, `credentialList`; `revokedCounts` |
| `OperationServices.find#Jobs`, `store#Job`, `find#JobRuns` | `jobName*`, `expectedRevision*`, `cronExpression`, `paused` → `jobList`; `applied`, `nextRun` |
| `OperationServices.find#Messages`, `apply#MessageRecovery` | System messages by `state`, `systemMessageTypeId`, `direction` → `messageList` |
| `OperationServices.find#MailOutcomes`, `retry#MailOutcome` | `state`, `emailMessageId`; `emailMessageId*`, `expectedRevision*` → `outcomeList`; `sent` |
| `OperationServices.find#Topics`, `store#TopicSubscription` | `topic*`, `username*`, `receiveNotifications`, `emailNotifications` |

`IdentityServices` and `OperationServices` also define the interfaces an application implements so its records show in administration: `define#OwnedResourceLookup`, `define#OwnedResourceTransfer`, `define#OwnedResourcePause`.

## Git: `org.moqui.git`

Repositories kept by the server (in the database, on disk, or a mirror of a remote), with reads, commits, refs, merges, branch rules, webhooks and merge requests. Rust (`moqui-git`, the `git` feature); merge requests and webhooks are partly inline. See [Git-Hosting](../interfaces/Git-Hosting.md).

| Service | |
|---|---|
| `GitServices.find#GitRepositories` | `text`, `repositoryKind`, `includeArchived`, `pageIndex`, `pageSize` → `repositoryList`, `repositoryListCount` |
| `GitServices.init#GitRepository`, `fetch#GitRepository` | `repositoryId*` → `headCommitId` |
| `GitServices.get#GitRefs` | `repositoryId*`, `refType` → `defaultBranch`, `headCommitId`, `refList` |
| `GitServices.get#GitLog` | `repositoryId*`, `revision`, `path`, `afterCommitId`, `limit` → `commitList`, `hasMore` |
| `GitServices.get#GitCommit` | `repositoryId*`, `commitId*`, `stats` → `commit`, `fileList`, `insertions`, `deletions` |
| `GitServices.get#GitTree` | `repositoryId*`, `revision`, `path` → `commitId`, `isFile`, `entryList` |
| `GitServices.get#GitBlob` | `repositoryId*`, `path*`, `revision` → `size`, `binary`, `text`, `bytes` |
| `GitServices.commit#GitFiles` | `repositoryId*`, `message*`, `fileMap`, `removePaths` → `commitId`: one commit on the default branch, as the current user |
| `GitServices.create#GitRef`, `delete#GitRef` | `repositoryId*`, `refName*`, `refType`, `revision`, `message` → `objectId`, `commitId` |
| `GitServices.get#GitCompare` | `repositoryId*`, `head*`, `base`, `withDiffs`, `checkMergeable` → `aheadBy`, `behindBy`, `commitList`, `fileList`, `mergeable` |
| `GitServices.merge#GitRefs` | `repositoryId*`, `source*`, `target*`, `method`, `message`, `checkOnly` → `commitId`, `mergeable`, `conflictPaths` |
| `GitServices.create#GitCommitStatus` | `repositoryId*`, `commitId*`, `state*`, `context`, `targetUrl`, `description`: a CI check |
| `GitServices.store#GitWebhook` | `repositoryId*`, `url*`, `events`, `secret`, `active` → `webhookId` |
| `GitReviewServices.create#MergeRequest` | `repositoryId*`, `sourceRef*`, `title*`, `targetRef`, `description`, `draft`, `reviewerUserIdList` → `mergeRequestId`, `mergeRequestNum` |
| `GitReviewServices.submit#MergeRequestReview`, `merge#MergeRequest`, `find#MergeRequests` | `mergeRequestId*`, `decision*`, `note` → `statusId`; `commitId`, `conflictPaths` |

For files, prefer the resource facade with a `git` location or a git drive ([Resources](#resources-orgmoquiresource)); these services are for the repository itself.

## Agents and AI tasks: `org.moqui.agent`

LLM tasks, agent runs, approvals, triggers, skills and the administration of models. Rust in `moqui-llm` (the `llm` feature), except the evaluation services (`AgentEval.js`). See [Assist](../ai/Assist.md), [A2A](../ai/A2A.md) and [MCP](../ai/MCP.md).

| Service | |
|---|---|
| `AiTaskServices.run#Summarize` | `text*`, `maxWords`, `focus` → `summary`, `modelId` |
| `AiTaskServices.run#Classify` | `text*`, `labelList*`, `multiple` → `label`, `labelList` |
| `AiTaskServices.run#Extract` | `text*`, `fieldMap` or `schema` → `valueMap` |
| `AiTaskServices.run#Translate` | `text*`, `targetLanguage*` → `translatedText` |
| `AiTaskServices.run#Generate` | `instructions*`, `text` → `generatedText` |
| `AiTaskServices.run#AiTask` | `taskType*`, `input`, `modelId` → `output`, `modelId`: a task of the registry (`list#AiTaskTypes`) |
| `AiTaskServices.propose#AiFieldValue` | `entityName*`, `fieldName*`, `pk` or `recordId`, `instructions` → `proposedValue`, `currentValue`, `reason` |
| `AgentServices.start#AgentRun` | `messageText*`, `agentId`, `conversationId`, `surface` → `runId`, `conversationId`, `statusId`, `queued` |
| `AgentServices.cancel#AgentRun`, `submit#AgentInput`, `get#AgentRunEvents` | `runId*`, `messageText`, `afterSeq` → `statusId`, `eventList` |
| `AgentServices.decide#AgentApproval` | `approvalId*`, `decision*`, `feedback`, `arguments` → `statusId` |
| `AgentTriggerServices.store#AgentTrigger`, `fire#AgentTrigger` | `agentId`, `triggerTypeEnumId`, `cronExpression`, `dataDocumentId`, `instructions` → `triggerId`; `runId` |
| `AgentAdminServices.check#LlmQuota` | `userId`, `scope` → `withinQuota`, `reason`, `usedTokens`, `maxTokens` |
| `AgentAdminServices.get#EffectiveConfig`, `test#Endpoint` | → `enabled`, `endpointList`, `modelList`, `bindingList`; `ok`, `latencyMs` |

Each `run#` task also takes `async` (answers `taskId`), `webhookUrl` and `allowInTransaction`. `AgentServices.get#UserFacts` is the interface of a user-facts provider. `AgentEvalServices` scores runs and runs experiments over datasets; `RemoteAgentServices` stores remote A2A agents and MCP servers.

## Edge and client sync: `org.moqui.edge`

`EdgeServices` is the link between a local node and its edge node: pairing, the queue between them, replication and delegated calls. Rust. `ClientSyncServices` is what an offline-capable client uses: calls made once per client reference, and uploads in chunks. Inline.

| Service | |
|---|---|
| `EdgeServices.get#LinkStatus` | → `nodeRole`, `state`, `url`, `rttMs`, `lastError`, `compatible` |
| `EdgeServices.create#PairingCode`, `pair#EdgeNode` | `validMinutes` → `code`; `url*`, `code*`, `expectFingerprint` |
| `EdgeServices.enqueue#Item` | `kind*`, `conversationKey*`, `idempotencyKey`, `payload`, `contentLocation` → `queueItemId`, `created`: an item for the other end, delivered in order once the link is up |
| `EdgeServices.get#QueueStatus` | `pairedNodeId` → `kindList`, `paused` |
| `ClientSyncServices.call#ClientRequest` | `serviceName*`, `clientRef`, `parameters` → `result`, `duplicate`: the call runs once per `clientRef` of the user |
| `ClientSyncServices.upload#Chunk` | `uploadId*`, `offset*`, `total*`, `fileName`, `contentType`, `chunk` → `received`, `complete` |
| `ClientSyncServices.get#UploadStatus`, `take#Upload` | `uploadId*` → `received`, `total`, `complete`; `location`, `fileName`, `fileSize` |

## Development and operations: `org.moqui.dev`

What the Tools and System apps call. Their reach is wide (SQL, conf files, restart), so each is gated by its own permissions; read a service's description before calling one from application code. Rust (`moqui-tools`, `moqui-devkit`, and the host's load runner), except `InstanceServices`.

| File | |
|---|---|
| `DatabaseServices` | The database tools: `get#Capabilities`, `compare#Schema` (`entityNames` → `tables`, `errors`), `run#Sql` (`sqlText*`, `parameters`, `allowChanges`, `token`, `limit` → `results`), `explain#Sql`, `get#TableStats`, `get#DatabaseActivity`, the data grid (`get#DataGridRows`, `export#DataGrid`), saved queries and connections ([Database](../tools/Database.md)) |
| `EtlServices` | Mappings that load a file into entities: `store#EtlMapping`, `check#EtlSample`, `run#EtlMapping` (`mappingId` or `mapping`, `location` or `contentBytes`, `wait` → `runId`, `statusId`, `loadCount`, `errorCount`), `get#EtlRuns` |
| `DevToolsServices` | The artifact index and XML edits: `find#Artifacts` (`text`, `kinds`, `components` → `artifacts`), `get#Artifact`, `get#ArtifactRefs` (`artifactId*`, `direction`), `apply#XmlOps`, `restart#Server` |
| `ComponentServices` | Conf and components on a running server: `reload#Conf`, `scan#Components`, `load#Component` (`componentName*`, `dataTypes` → `report`), `load#ComponentData`, `deploy#Component`, `update#Component` ([Component-Deploy](../tools/Component-Deploy.md)) |
| `LoadServices` | Load runs from Tools: `start#LoadRun` (`scriptLocation*`, `targetUrl`, `parameters` → `runId`), `get#LoadRunStatus`, `compare#LoadRuns` ([Load-Runner](../tools/Load-Runner.md)) |
| `SimServices` | `run#Steps` (`steps`, `read`, `label` → `steps`, `readResult`): service calls one after another in a sim run |
| `InstanceServices` | Provisioning application instances and their databases: `provision#AppInstance` (`appInstanceId`), and the interfaces `init#Instance`, `start#Instance`, `create#Database` with Docker, MySQL and Postgres implementations |

## Comms screens: `basic-comms`

`runtime/base-component/basic-comms/service/org/moqui/comms/` holds the services of the Comms app's screens. They are wrappers over the mail, chat and file services that answer in the shape a screen wants, so server code usually calls the framework service and a screen or a client calls these. Inline actions. See [Comms](../tools/Comms.md).

| Service | |
|---|---|
| `InboxServices.get#InboxData` | → `accountList`, `mailboxList`, `searchList`, `totals`, `prefs` |
| `InboxServices.find#InboxRows` | `view`, `emailAccountId`, `emailFolderId`, `searchText`, `unreadOnly`, `cursor`, `pageSize` → `threadList`, `hasMore`, `nextCursor` |
| `InboxServices.get#ThreadView` | `store*`, `threadId*` → `thread` |
| `InboxServices.set#ThreadState` | `targetList`, `action*` (read, unread, flag, ...), `snoozedThruDate`, `emailFolderId` → `updatedCount`, `failedList` |
| `ComposeServices.get#ComposeData` | `emailMessageId`, `replyToEmailMessageId`, `forwardEmailMessageId`, `to`, `subject`, `recordEntity`, `recordId` → `compose` |
| `ComposeServices.save#ComposeDraft`, `send#Compose` | `fromValue*`, `toJson`, `ccJson`, `subject`, `body`, `scheduledSendDate` → `emailMessageId`, `sent`, `sendError` |
| `ComposeServices.search#Recipients` | `term`, `mode`, `limit` → `optionList` |
| `ChatScreenServices.get#ChatData` | `limit` → `me`, `prefs`, `roomList`, `unreadTotal`, `mentionTotal` |
| `ChatScreenServices.find#RecordRoom` | `recordEntityName*`, `recordId*` → `chatRoomId`, `created`: the room of a record, made on first use |
| `ChatScreenServices.send#ChatMessage`, `create#ChatRoom` | As the chat services, from the composer and dialogs |
| `FilePickerServices.get#PickerData`, `list#PickerFolder` | `driveId*`, `path` → `driveList`, `recentList`; `fileList`, `breadcrumbList` |
| `CommsServices.search#Users` | `term`, `limit` → `userList` of `{id, value, label}` |
| `CommsHomeServices.get#CommsHome` | → `cardList`, `needsReplyList`, `followUpList`, `meetingList`, `roomList` |

`ProviderInterfaces.xml` holds the interfaces an application implements to take part in Comms through shell providers: `inbox#Source`, `recipient#Source`, `record#Lookup`, `record#Link`, `create#From`, `contact#Card`, `home#Source`, `call#Target` and the others. `MailSettingsServices` is the mail settings screens.

## Tools: `tools`

`runtime/base-component/tools/service/moqui/tools/` has only a few services of the Tools and System home screens: `SystemHomeServices.get#SystemAttention` (→ `items`: what needs an administrator), `SystemHomeServices.get#CounterSeries`, `ToolsHomeServices.get#ScreenCount` and `DatabaseLoadServices.run#LoadWrap`. Their names start `moqui.tools.`. The services behind the Tools screens are the [`org.moqui.dev`](#development-and-operations-orgmoquidev) group. `tools/service/moqui.rest.xml` is the REST mapping of the framework's services.
