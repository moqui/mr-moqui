// The host surface of plans/ContextObjects.md (Server host methods) and the browser host of plans/QvtServerStatic.md.
// This part does not depend on the definitions. `moqui types` writes it beside the generated files.
// Declarations only: the QuickJS host does not read this file and does not enforce it.

// ---- host values ----

/**
 * A decimal on the server: a `rust_decimal` host value, not a JavaScript number. Operators are not overloaded, so
 * `amount + tax` and `amount * tax` are type errors here. `==` and `if (amount)` still compile; `moqui lint` flags them.
 * In the unwrapped projection, the locked-down template and the browser a decimal is a string.
 */
interface Decimal {
    add(other: Decimal | number | string): Decimal;
    subtract(other: Decimal | number | string): Decimal;
    multiply(other: Decimal | number | string): Decimal;
    /** Scale `max(left, right, 10)`. */
    divide(other: Decimal | number | string): Decimal;
    eq(other: Decimal | number | string): boolean;
    /** -1, 0 or 1. */
    compare(other: Decimal | number | string): number;
    isZero(): boolean;
    abs(): Decimal;
    negate(): Decimal;
    /** The digits after the decimal point. */
    scale(): number;
    /** Java's `divide(other, scale, roundingMode)`: the quotient rounded to `scale` places. The mode is a `RoundingMode` name or an old `ROUND_*` number. */
    divide(other: Decimal | number | string, scale: number, roundingMode?: string | number | null): Decimal;
    /** Java's `setScale`. Without a mode a value that needs rounding is an error. */
    setScale(scale: number, roundingMode?: string | number | null): Decimal;
    /** A JavaScript number, with float rounding. */
    toNumber(): number;
    toString(): string;
    /** Throws an error that names the method to use. */
    valueOf(): never;
}

/** A timestamp: ISO-8601 text, branded so a plain string is not taken for one. */
type Timestamp = string & { readonly __moquiTimestamp: never };

/** A value that converts to a decimal parameter: the host value, or text and numbers the validation converts. */
type DecimalInput = Decimal | number | string;
/** A value that converts to a timestamp parameter. */
type TimestampInput = Timestamp | string | number;

/** A plain map with no proven keys. */
type MoquiMap = Record<string, unknown>;

/** The context stack as a map. A name no map holds reads as null on the server. */
interface MoquiContext {
    [name: string]: unknown;
}

// ---- entities ----

/** Filled by the generated `entities.d.ts`: one entry for each entity by full name, unique entity name, and short alias. */
interface MoquiEntityMap {}

/** What the generator knows of an entity: field types as the host's get returns them, primary key names, relationships. */
interface MoquiEntityShape {
    fields: object;
    pk: string;
    rel: object;
}

type MoquiEntityOf<N extends string> = N extends keyof MoquiEntityMap ? MoquiEntityMap[N] : never;
/** The field types of entity `N`, or a plain map when the entity is not loaded. */
type MoquiFields<N extends string> = [MoquiEntityOf<N>] extends [never]
    ? MoquiMap
    : MoquiEntityOf<N> extends MoquiEntityShape
      ? MoquiEntityOf<N>["fields"]
      : MoquiMap;
type MoquiFieldName<N extends string> = keyof MoquiFields<N> & string;
type MoquiPk<N extends string> = [MoquiEntityOf<N>] extends [never]
    ? string
    : MoquiEntityOf<N> extends MoquiEntityShape
      ? MoquiEntityOf<N>["pk"] & string
      : string;
type MoquiRel<N extends string> = [MoquiEntityOf<N>] extends [never]
    ? Record<string, string>
    : MoquiEntityOf<N> extends MoquiEntityShape
      ? MoquiEntityOf<N>["rel"]
      : Record<string, string>;
type MoquiRelated<N extends string, R extends string> = R extends keyof MoquiRel<N>
    ? MoquiRel<N>[R] & string
    : string;

/** What a field set accepts: the entity layer converts text to the field type on write. */
type MoquiFieldInput<T> = T extends Decimal
    ? DecimalInput | null
    : T extends Timestamp
      ? TimestampInput | null
      : T extends number
        ? number | string | null
        : T extends boolean
          ? boolean | string | null
          : T | null;

/**
 * An entity value host object. Fields are read and written as properties (`value.statusId`, `value["statusId"]`) and
 * with `get` and `set`, as the QuickJS host exposes them. A method wins over a field of the same name. A relationship
 * name also reads as a property, the related value or list; use `findRelated`/`findRelatedOne` for a typed result.
 */
type EntityValue<N extends string = string> = EntityValueMethods<N> & MoquiFieldProperties<N>;
/** The fields of entity `N` as properties, except a name a method has. */
type MoquiFieldProperties<N extends string> = {
    [K in Exclude<MoquiFieldName<N>, keyof EntityValueMethods<N>>]: MoquiFields<N>[K] | null;
};
interface EntityValueMethods<N extends string = string> {
    get<K extends MoquiFieldName<N>>(field: K): MoquiFields<N>[K] | null;
    set<K extends MoquiFieldName<N>>(field: K, value: MoquiFieldInput<MoquiFields<N>[K]>): void;
    setFields(fields: MoquiMap, setIfEmpty?: boolean, namePrefix?: string | null, pks?: boolean | null): void;
    setSequencedIdPrimary(): void;
    setSequencedIdSecondary(): void;
    /** Reads the row of the primary key again and replaces the fields. False, and unchanged, when there is no row. */
    refresh(): boolean;
    /** Each field as an entry: `{key, value, getKey(), getValue()}`. */
    entrySet(): { key: string; value: unknown; getKey(): string; getValue(): unknown }[];
    /** The short entity name in words (`OrderPart` is `Order Part`). */
    getEntityNamePretty(): string;
    resolveEntityNamePretty(): string;
    /** The rows of every entity that holds a foreign key to this value, as one list. Skip by full name, short alias or entity name; an array or a Set. */
    findRelatedFk(skipEntities?: string[] | Set<string> | null): EntityList;
    getMap(): MoquiMap;
    getEntityName(): string;
    resolveEntityName(): string;
    create(): void;
    createOrUpdate(): void;
    update(): void;
    delete(): void;
    deleteRelated(relationship: keyof MoquiRel<N> & string): void;
    /** Deletes the rows of the named `many` relationships (names or short aliases), then this value. False, with nothing deleted, when rows of another entity still reference it. */
    deleteWithRelated(relationships?: string[] | Set<string> | null): boolean;
    findRelated<R extends keyof MoquiRel<N> & string>(
        relationship: R,
        byAndFields?: MoquiMap | null,
        orderBy?: string[] | null,
        useCache?: boolean | null,
        forUpdate?: boolean | null,
    ): EntityList<MoquiRelated<N, R>>;
    /** The primary key fields of the value. */
    getPrimaryKeys(): { [K in MoquiPk<N> & MoquiFieldName<N>]?: MoquiFields<N>[K] };
    findRelatedOne<R extends keyof MoquiRel<N> & string>(
        relationship: R,
        useCache?: boolean | null,
        forUpdate?: boolean | null,
    ): EntityValue<MoquiRelated<N, R>> | null;
}

/** An entity list host object: index, length, iterable, `map` and `filter`. */
interface EntityList<N extends string = string> extends Iterable<EntityValue<N>> {
    readonly [index: number]: EntityValue<N>;
    readonly length: number;
    readonly pageIndex: number;
    readonly pageSize: number;
    /** The rest of the Array prototype runs on an array copy of the rows, so `sort` and `reverse` return the reordered copy and leave the list. */
    find(callback: (value: EntityValue<N>, index: number) => unknown): EntityValue<N> | undefined;
    findIndex(callback: (value: EntityValue<N>, index: number) => unknown): number;
    some(callback: (value: EntityValue<N>, index: number) => unknown): boolean;
    every(callback: (value: EntityValue<N>, index: number) => unknown): boolean;
    reduce<T>(callback: (accumulator: T, value: EntityValue<N>, index: number) => T, initial: T): T;
    sort(compare?: (left: EntityValue<N>, right: EntityValue<N>) => number): EntityValue<N>[];
    includes(value: EntityValue<N>): boolean;
    indexOf(value: EntityValue<N>): number;
    slice(start?: number, end?: number): EntityValue<N>[];
    concat(...items: unknown[]): unknown[];
    join(separator?: string): string;
    forEach(callback: (value: EntityValue<N>, index: number) => void): void;
    get(index: number): EntityValue<N> | null;
    size(): number;
    getPartialList(offset: number, size: number, countLimit?: boolean): EntityList<N>;
    filterByDate(
        fromField?: string | null,
        thruField?: string | null,
        moment?: TimestampInput | null,
        ignoreIfEmpty?: boolean,
    ): EntityList<N>;
    filterByLimit(offset: number, limit: number): EntityList<N>;
    map<T>(callback: (value: EntityValue<N>, index: number) => T): T[];
    filter(callback: (value: EntityValue<N>, index: number) => boolean): EntityValue<N>[];
}

/** A condition made by `ec.entity.conditionFactory`. */
interface EntityCondition {
    readonly __moquiCondition: never;
    /** Compare text without regard to case. */
    ignoreCase(): EntityCondition;
}

/** A find builder. Each chain method returns the builder; a builder runs once. */
interface EntityFind<N extends string = string> {
    condition(field: MoquiFieldName<N>, value: unknown): EntityFind<N>;
    /** The operator is a name of Java's factory: `equals`, `not-equals`, `less-than`, `greater`, `<=`, `like`, `in`, `between`, `is-null` and the rest. An unknown one is an error. */
    condition(field: MoquiFieldName<N>, operator: string | null, value: unknown): EntityFind<N>;
    /** Rows in effect at the moment (now when empty): from is null or not after it, thru is null or after it. */
    conditionDate(fromField: string, thruField: string, moment?: TimestampInput | null): EntityFind<N>;
    condition(fields: MoquiMap | null): EntityFind<N>;
    condition(condition: EntityCondition | null): EntityFind<N>;
    havingCondition(condition: EntityCondition | null): EntityFind<N>;
    searchFormMap(inputs: MoquiMap, defaultParameters?: MoquiMap | null, skipFields?: string | null, ...rest: unknown[]): EntityFind<N>;
    requireSearchFormParameters(require?: boolean): EntityFind<N>;
    useCache(flag?: boolean | null): EntityFind<N>;
    forUpdate(flag?: boolean | null): EntityFind<N>;
    useClone(flag?: boolean | null): EntityFind<N>;
    distinct(flag?: boolean | null): EntityFind<N>;
    disableAuthz(): EntityFind<N>;
    offset(offset: number | null): EntityFind<N>;
    /** The offset `pageIndex * pageSize`; Java's does not set the limit. */
    offset(pageIndex: number, pageSize: number): EntityFind<N>;
    /** Set the fields on each row found and update the ones that change. How many were updated. */
    updateAll(fieldsToSet: MoquiMap): number;
    limit(limit: number | null): EntityFind<N>;
    selectField(field: string): EntityFind<N>;
    /** Comma separated fields; `-` descending, `^` case-insensitive. */
    orderBy(fields: string): EntityFind<N>;
    copy(): EntityFind<N>;
    one(): EntityValue<N> | null;
    /** A plain master graph retaining nested identities; `default` when omitted. */
    oneMaster(master?: string): MoquiMap | null;
    list(): EntityList<N>;
    iterator(): EntityList<N>;
    count(): number;
    /** How many rows were deleted. */
    deleteAll(): number;
    getLimit(): number | null;
    getOffset(): number | null;
    /** `offset / pageSize`, 0 with no offset. */
    getPageIndex(): number;
    /** The limit, 20 with none. */
    getPageSize(): number;
}

// ---- services ----

interface MoquiParameterInfo {
    name: string;
    type: string;
    required: boolean;
}

/** Filled by the generated `services.d.ts`: input and output maps by service name. */
interface MoquiServiceIn {}
interface MoquiServiceOut {}

type MoquiAutoVerb = "create" | "store" | "update" | "delete";
type MoquiAutoInput<N extends string> = { [K in MoquiFieldName<N>]?: MoquiFieldInput<MoquiFields<N>[K]> } & {
    [R in keyof MoquiRel<N> & string]?: MoquiMap | MoquiMap[];
};
/**
 * The implicit entity-auto verbs. The validation of an implicit service declares no parameter; the runner needs the
 * primary key for `update` and `delete`, so those are required here. Nested related records go under a relationship name.
 */
type MoquiEntityAutoIn<V extends MoquiAutoVerb, N extends string> = V extends "update" | "delete"
    ? { [K in MoquiPk<N>]: MoquiFieldInput<MoquiFields<N>[K & MoquiFieldName<N>]> } & MoquiAutoInput<N>
    : MoquiAutoInput<N>;
/** What an entity-auto service returns: the primary key of the record. */
type MoquiEntityAutoOut<N extends string> = { [K in MoquiPk<N> & MoquiFieldName<N>]?: MoquiFields<N>[K] };

type MoquiAllEntityAutoIn = {
    [E in keyof MoquiEntityMap & string as `create#${E}`]: MoquiEntityAutoIn<"create", E>;
} & {
    [E in keyof MoquiEntityMap & string as `store#${E}`]: MoquiEntityAutoIn<"store", E>;
} & {
    [E in keyof MoquiEntityMap & string as `update#${E}`]: MoquiEntityAutoIn<"update", E>;
} & {
    [E in keyof MoquiEntityMap & string as `delete#${E}`]: MoquiEntityAutoIn<"delete", E>;
};

/** The input map of service `S`: a defined service, an implicit entity-auto verb, or a plain map for a name not loaded. */
type MoquiIn<S extends string> = S extends keyof MoquiServiceIn
    ? MoquiServiceIn[S]
    : S extends `${infer V}#${infer E}`
      ? V extends MoquiAutoVerb
          ? E extends keyof MoquiEntityMap
              ? MoquiEntityAutoIn<V, E>
              : MoquiMap
          : MoquiMap
      : S extends `${infer V}-${infer E}`
        ? V extends MoquiAutoVerb
            ? E extends keyof MoquiEntityMap
                ? MoquiEntityAutoIn<V, E>
                : MoquiMap
            : MoquiMap
        : MoquiMap;
type MoquiOut<S extends string> = S extends keyof MoquiServiceOut
    ? MoquiServiceOut[S]
    : S extends `${infer V}#${infer E}`
      ? V extends MoquiAutoVerb
          ? MoquiEntityAutoOut<E>
          : MoquiMap
      : MoquiMap;
/** Service names an editor completes. Any other string is accepted with a plain map. */
type MoquiServiceName = keyof MoquiServiceIn | keyof MoquiAllEntityAutoIn | (string & {});

interface ServiceCallOptions<Self> {
    disableAuthz(): Self;
    multi(flag?: boolean): Self;
    ignoreTransaction(flag?: boolean): Self;
    requireNewTransaction(flag?: boolean): Self;
    transactionTimeout(seconds: number | null): Self;
    useTransactionCache(flag?: boolean): Self;
    /** Input that fails validation is danger messages, not errors, and the call returns no results. */
    softValidate(flag?: boolean): Self;
    /** Java sends the call to another server of the cluster; here it is the same call. */
    distribute(flag?: boolean): Self;
}

interface ServiceCallSync extends ServiceCallOptions<ServiceCallSync> {
    name<S extends MoquiServiceName & string>(name: S): ServiceCallSyncNamed<S>;
}

interface ServiceCallSyncNamed<S extends string> extends ServiceCallOptions<ServiceCallSyncNamed<S>> {
    parameters(parameters: MoquiIn<S>): ServiceCallSyncNamed<S>;
    parameter<K extends keyof MoquiIn<S> & string>(name: K, value: MoquiIn<S>[K]): ServiceCallSyncNamed<S>;
    call(): MoquiOut<S>;
}

interface ServiceCallAsync extends ServiceCallOptions<ServiceCallAsync> {
    name<S extends MoquiServiceName & string>(name: S): ServiceCallAsyncNamed<S>;
}

interface ServiceCallAsyncNamed<S extends string> extends ServiceCallOptions<ServiceCallAsyncNamed<S>> {
    parameters(parameters: MoquiIn<S>): ServiceCallAsyncNamed<S>;
    parameter<K extends keyof MoquiIn<S> & string>(name: K, value: MoquiIn<S>[K]): ServiceCallAsyncNamed<S>;
    call(): void;
    callFuture(): ServiceResultFuture<S>;
}

interface ServiceResultFuture<S extends string> {
    isDone(): boolean;
    get(timeoutMillis?: number): MoquiOut<S>;
}

// ---- server ec: the facade surface ----

interface MoquiEntityFacade {
    find<N extends (keyof MoquiEntityMap & string) | (string & {})>(entityName: N): EntityFind<N>;
    makeValue<N extends (keyof MoquiEntityMap & string) | (string & {})>(entityName: N): EntityValue<N>;
    getDataDocuments(...args: unknown[]): MoquiMap[];
    /** Flatten a graph to unpersisted values, inheriting keys and skipping read-only relationships. */
    getValueListFromPlainMap(graph: MoquiMap, entityName: string): EntityList;
    /** The next id of a sequence name, as text. No stagger is 0, no bank size 50. */
    sequencedIdPrimary(seqName: string, staggerMax?: number | null, bankSize?: number | null): string;
    /** Pin the next primary ids of an entity or sequence name to `nextSeqNum` through `nextSeqNum + bankSize`, factory wide. For tests. */
    tempSetSequencedIdPrimary(seqName: string, nextSeqNum: number, bankSize: number): void;
    /** The next id of the sequence comes from the database again. */
    tempResetSequencedIdPrimary(seqName: string): void;
    /** The time zone of the database timestamps: UTC. Accepted and ignored as the fourth argument of `ec.l10n.format`. */
    readonly databaseTimeZone: string;
    readonly conditionFactory: {
        /** The fourth argument is Java's `orNull` (or the field is null); the fifth makes the comparison ignore case. `condition.ignoreCase()` does the same. */
        makeCondition(field: string, operator: string, value: unknown, orNull?: boolean, ignoreCase?: boolean): EntityCondition;
        /** A SQL fragment used as written, as in a view entity's aliases; null for empty text. */
        makeConditionWhere(sql: string): EntityCondition | null;
        makeCondition(conditions: EntityCondition[], joinOperator?: string): EntityCondition;
        makeCondition(fields: MoquiMap): EntityCondition;
        makeConditionDate(
            fromField: string | null,
            thruField: string | null,
            moment: TimestampInput | null,
            ignoreIfEmpty?: boolean,
            ignore?: string | boolean | null,
        ): EntityCondition | null;
        makeActionConditionDirect(
            field: string,
            operator: string | null,
            value: unknown,
            toField: string | null,
            ignoreCase: boolean | null,
            ignoreIfEmpty: boolean | null,
            orNull: boolean | null,
            not: boolean | null,
            ignore: string | boolean | null,
        ): EntityCondition | null;
    };
}

interface MoquiResourceFacade {
    /** Localizes text shorter than 256 characters, then expands `${}` against the context and `map`. */
    expand(text: string, location?: string | null, map?: MoquiMap | null): string;
    expandNoL10n(text: string, location?: string | null, map?: MoquiMap | null): string;
    expression(expression: string, location?: string | null, map?: MoquiMap | null): unknown;
    condition(expression: string, location?: string | null, map?: MoquiMap | null): boolean;
    script(location: string): void;
    template(location: string, data?: MoquiMap | null): string;
    /** PDF bytes of an `html-print` (old name `xsl-fo`) document. `xslt` must be null and `out` is ignored. Throws when PDF is not available. */
    xslFoTransform(document: string, xslt?: null, out?: unknown, contentType?: string | null): Uint8Array;
    /** The Code 128 barcode of `text` as an inline `<svg>` sized in millimetres, for a print document. Lengths take CSS units
     * (or millimetres as a number); `height` includes the text under the bars. No text, or text outside ASCII, gives "". */
    code128Svg(
        text: unknown,
        options?: {
            height?: string | number;
            moduleWidth?: string | number;
            fontSize?: string | number;
            fontFamily?: string;
            humanReadable?: boolean;
            quietZone?: number | boolean;
        } | null,
    ): string;
    /** A change set under one root (docs/Revision-Management.md, Change sets): one commit on git://, the changes in order elsewhere. */
    makeChangeSet(root: string): MoquiChangeSet;
    /** Write with a note for the version (the commit message on git://, versionNote on dbresource://). */
    putContent(location: string, content: unknown, note?: string | null): void;
    /** The last change of a file or directory; on git:// from the path index. Null when nothing is there. */
    getLastChange(location: string): MoquiResourceChange | null;
    getChildChanges(location: string): (MoquiResourceChange & { name: string })[];
    /** The versions under a location, newest first; on git:// each commit under the path with the paths it changed. */
    getChangeLog(location: string, limit?: number | null): { versionName: string; previousVersionName: string | null; versionDate: unknown; userId: string | null; note: string | null; paths: string[] }[];
    getTreeSize(location: string): number;
    /** A version written back as the newest; the new version's name. */
    restoreVersion(location: string, versionName: string, note?: string | null): string | null;
    getVersionBytes(location: string, versionName: string): Uint8Array | null;
    contentTypeOf(filename: string): string | null;
    /** A PNG thumbnail of an image, no side over maxSize (256); null when the bytes are not an image. */
    makeThumbnail(source: string, target: string, maxSize?: number | null): { width: number; height: number; contentType: string } | null;
    /** Page 1 of a PDF as a PNG at target, no side over maxSize (1024), with the page count; null when there is no PDF engine or the PDF does not open. */
    makePdfPreview(source: string, target: string, maxSize?: number | null): { width: number; height: number; pages: number; contentType: string } | null;
    /** The first rows of a sheet (the first when sheetName is empty), from the first used row and column; null when it is not a spreadsheet. */
    readSheetPreview(location: string, sheetName?: string | null, maxRows?: number | null, maxCols?: number | null): { sheetNames: string[]; sheetName: string; rows: unknown[][]; truncated: boolean } | null;
    /** The text of a PDF, spreadsheet or text file (maxChars, 100000), kept under runtime://tmp/file-text by cacheKey; "" when it has none; null with cacheOnly when not extracted yet. */
    extractText(location: string, cacheKey: string, maxChars?: number | null, cacheOnly?: boolean | null): string | null;
    /** A zip of the files at the entries' locations, each under its path; an entry that cannot be read is in failures. */
    writeZip(target: string, entries: { path: string; location: string }[]): { count: number; size: number; failures: { path: string; error: string }[] };
    /** A signed grant for subject, valid ttlSeconds (3600); checkGrant verifies it with the same key. */
    makeGrant(key: string, subject: string, ttlSeconds?: number | null): string;
    checkGrant(key: string, subject: string, token: string | null): boolean;
}

interface MoquiChangeSet {
    write(path: string, content: unknown): MoquiChangeSet;
    delete(path: string): MoquiChangeSet;
    move(from: string, to: string): MoquiChangeSet;
    mkdir(path: string): MoquiChangeSet;
    /** The versions written are by userId at date (a timestamp or milliseconds); the committer stays the current user. */
    author(userId: string | null, date?: unknown): MoquiChangeSet;
    size(): number;
    commit(message?: string | null): { versionName: string | null; applied: string[]; failures: { path: string; error: string }[] };
}

interface MoquiResourceChange {
    isFile: boolean;
    size: number | null;
    versionName: string | null;
    /** Milliseconds since the epoch. */
    date: number | null;
    userId: string | null;
    note: string | null;
}

interface MoquiL10nFacade {
    localize(text: string): string;
    format(value: unknown, pattern?: string | null): string;
    roundCurrency(amount: unknown, uomId?: string | null, precise?: boolean, roundingMode?: string | number | null): Decimal | null;
    formatCurrency(amount: unknown, uomId?: string | null, fractionDigits?: number | null): string;
    formatCurrencyNoSymbol(amount: unknown, uomId?: string | null, fractionDigits?: number | null): string;
    parseTimestamp(text: string, format?: string | null): Timestamp | null;
}

interface MoquiMessageFacade {
    addError(message: string): void;
    addMessage(message: string, type?: string): void;
    addPublic(message: string, type?: string): void;
    hasError(): boolean;
    getErrorsString(): string;
    readonly errorsString: string;
    /** Clear active errors and validation errors, retaining informational messages. */
    clearErrors(): void;
    pushErrors(): void;
    popErrors(): void;
    readonly errors: string[];
}

interface MoquiLoggerFacade {
    log(level: string | number, message: string, thrown?: unknown): void;
    warn(message: string): void;
}

interface MoquiUserFacade {
    readonly userId: string | null;
    readonly username: string | null;
    readonly locale: string;
    getLocale(): string;
    readonly timeZone: string;
    getTimeZone(): string;
    /** The signed-in `UserAccount` without its credential columns. */
    readonly userAccount: MoquiMap | null;
    /** The effective time when one is set, else now. */
    readonly nowTimestamp: Timestamp;
    /** `nowTimestamp`, date filters with no date, and the entity-auto `fromDate` and audit log stamps use this time for this execution context; null clears it. */
    setEffectiveTime(time: TimestampInput | null): void;
    readonly passwordRules: { minLength: number; minDigits: number; minOthers: number };
    getPreference(key: string): string | null;
    /** The `moqui.server.Visit` id of this session, null outside one. */
    readonly visitId: string | null;
    getVisitId(): string | null;
    /** The `moqui.server.Visit` of this session as a map, null outside one. */
    readonly visit: MoquiMap | null;
    getVisit(): MoquiMap | null;
    /** End another session of the signed-in user by its visit (`org.moqui.impl.UserServices.end#UserVisit`); the web sessions ended. */
    endVisit(visitId: string): number;
    /** A null or empty username or password fails as a wrong one does: false, with the message in ec.message. */
    loginUser(username: string | null, password: string | null): boolean;
    internalLoginUser(username: string): boolean;
    logoutUser(): boolean;
}

/** An uploaded file. A script cannot hold the bytes. */
interface MoquiUploadedFile {
    readonly name: string;
    readonly fileName: string;
    readonly fieldName: string;
    readonly contentType: string;
    readonly size: number;
    /** The bytes as text in the charset (Java names: UTF-8, ISO-8859-1, windows-1252, UTF-16), UTF-8 by default. */
    getString(charset?: string): string;
    /** `dbresource://` only. */
    write(location: string): void;
    isInMemory(): boolean;
}

interface MoquiPageMessage {
    message: string;
    typeString: string;
}

interface MoquiWebFacade {
    readonly parameters: MoquiMap;
    readonly requestParameters: MoquiMap;
    readonly multiPartParameters: MoquiMap;
    readonly sessionAttributes: MoquiMap;
    readonly sessionToken: string;
    readonly savedMessages: string[];
    readonly savedErrors: string[];
    readonly savedValidationErrors: unknown[];
    readonly errorParameters: MoquiMap | null;
    sendJsonResponse(value: unknown): void;
    sendJsonError(status: number, message: string, error?: unknown): void;
    /** HTML, Java's single-quoted JSON, or plain text, from `Accept`. */
    sendError(errorCode: number, message?: string | null, throwable?: { message?: string } | null): void;
    /** text/plain by default; a file name sends the text as an attachment of that name. */
    sendTextResponse(text: string | null, contentType?: string | null, filename?: string | null): void;
    /** The bytes at a resource location, inline or (the default) as an attachment. 404 when nothing is there. */
    sendResourceResponse(location: string, inline?: boolean): void;
    saveScreenLastInfo(path?: string | null, parameters?: MoquiMap | null): void;
    getPageMessages(): MoquiPageMessage[];
    getScreenHistory(): { name: string; url: string; urlNoParams: string }[];
    getHostName(withPort?: boolean): string;
    getResourceDistinctValue(): string;
    /** Empty when a path-only link will do (in a request whose security matches useEncryption), else the scheme and
     * host: the render's base URL, or the conf webapp's host and port, never the request's Host header. */
    getWebappRootUrl(requireFullUrl?: boolean, useEncryption?: boolean | null): string;
    getWebappRootUrl(webappName: string, servletContextPath: string | null, requireFullUrl: boolean,
        useEncryption?: boolean | null): string;
    /** The servlet API's request of a web request, null with none (docs/Web-Scripts.md). */
    readonly request: MoquiWebRequest | null;
    /** The servlet API's response of a web request, null with none. */
    readonly response: MoquiWebResponse | null;
    getRequest(): MoquiWebRequest | null;
    getResponse(): MoquiWebResponse | null;
}

/** A subset of `HttpServletRequest`; each field is also a getX() method, `secure` is isSecure(). */
interface MoquiWebRequest {
    readonly method: string;
    readonly requestURI: string;
    readonly queryString: string | null;
    readonly contextPath: string;
    readonly servletPath: string;
    readonly pathInfo: string | null;
    readonly scheme: string;
    readonly serverName: string;
    readonly serverPort: number;
    readonly secure: boolean;
    readonly remoteAddr: string;
    readonly contentType: string | null;
    readonly characterEncoding: string | null;
    readonly contentLength: number;
    /** The request headers as sent, names in lower case. */
    readonly headerPairs: [string, string][];
    getRequestURL(): string;
    getHeader(name: string): string | null;
    getHeaders(name: string): string[];
    getHeaderNames(): string[];
    getCookies(): { name: string; value: string }[];
    getParameter(name: string): string | null;
    getParameterValues(name: string): string[] | null;
    getParameterMap(): MoquiMap;
    getParameterNames(): string[];
    getAttribute(name: string): unknown;
    setAttribute(name: string, value: unknown): void;
    removeAttribute(name: string): void;
    getAttributeNames(): string[];
    getSession(): { id: string; getId(): string; getAttribute(name: string): unknown; setAttribute(name: string, value: unknown): void;
        removeAttribute(name: string): void; getAttributeNames(): string[] } | null;
    /** The raw body as text; empty for a multipart body. */
    bodyText(): string;
    bodyBytes(): Uint8Array;
}

/** A subset of `HttpServletResponse`. */
interface MoquiWebResponse {
    setStatus(status: number): void;
    getStatus(): number;
    setHeader(name: string, value: string | null): void;
    addHeader(name: string, value: string): void;
    getHeader(name: string): string | null;
    getHeaders(name: string): string[];
    getHeaderNames(): string[];
    containsHeader(name: string): boolean;
    setContentType(contentType: string): void;
    getContentType(): string | null;
    setCharacterEncoding(charset: string): void;
    write(content: string | Uint8Array): void;
    getWriter(): { write(text: string): void; print(text: string): void; println(text?: string): void; flush(): void };
    getOutputStream(): { write(bytes: Uint8Array | string): void; flush(): void };
    isCommitted(): boolean;
    sendError(status: number, message?: string | null): void;
    sendRedirect(location: string): void;
    /** The status, headers and body of an `ec.service.rest()` response as this response; a body `send()` left on the server
     * streams to the client without entering the script. */
    forward(upstream: unknown, options?: { skipHeaders?: string[] }): void;
}

/** The `config` of a web script handler: its conf element's name and init-params. */
interface MoquiWebHandlerConfig {
    readonly name: string;
    readonly location: string;
    readonly initParams: Record<string, string>;
    getInitParameter(name: string): string | null;
    getInitParameterNames(): string[];
}

interface MoquiScreenRenderOptions {
    screenPath: string;
    renderMode?: string;
    rootScreen?: string;
    baseLinkUrl?: string;
    lastStandalone?: string;
    parameters?: MoquiMap;
}

interface MoquiScreenRenderBuilder {
    webappName(name: string): MoquiScreenRenderBuilder;
    rootScreen(location: string): MoquiScreenRenderBuilder;
    /** Ignored. */
    rootScreenFromHost(host: string): MoquiScreenRenderBuilder;
    screenPath(path: string): MoquiScreenRenderBuilder;
    renderMode(mode: string): MoquiScreenRenderBuilder;
    baseLinkUrl(url: string): MoquiScreenRenderBuilder;
    lastStandalone(value: string): MoquiScreenRenderBuilder;
    parameters(parameters: MoquiMap): MoquiScreenRenderBuilder;
    render(): string;
    /** The document of a binary mode such as xlsx, or the UTF-8 of a text mode. */
    renderBytes(): Uint8Array;
}

/** One render of a `ScreenTest`. A failed render does not throw: its errors are in `errorMessages`. */
interface MoquiScreenTestRender {
    readonly screenPath: string;
    /** The response text, null when the render raised an error. */
    readonly output: string | null;
    readonly errorMessages: string[];
    readonly renderTime: number;
    readonly status: number | null;
    /** The output parsed as JSON, null when it is not JSON. */
    readonly jsonObject: unknown;
    assertContains(text: string): boolean;
    assertNotContains(text: string): boolean;
    assertNoErrors(): boolean;
    getOutput(): string | null;
    getErrorMessages(): string[];
    getRenderTime(): number;
}

interface MoquiScreenTest {
    rootScreen(location: string): MoquiScreenTest;
    /** Render paths are under this path of the root screen, such as `rest` or `apps/my`. */
    baseScreenPath(path: string): MoquiScreenTest;
    renderMode(mode: string): MoquiScreenTest;
    baseLinkUrl(url: string): MoquiScreenTest;
    /** Render as the current user; `requestMethod` is GET when not given. */
    render(screenPath: string, parameters?: MoquiMap | null, requestMethod?: string | null): MoquiScreenTestRender;
    renderAll(screenPaths: string[], parameters?: MoquiMap | null, requestMethod?: string | null): void;
    readonly renderCount: number;
    readonly errorCount: number;
    readonly renderTotalChars: number;
    /** Epoch millis when the test was made. */
    readonly startTime: number;
    getRenderCount(): number;
    getErrorCount(): number;
    getRenderTotalChars(): number;
    getStartTime(): number;
}

interface MoquiScreenFacade {
    /** Read metadata only; throws for missing or ambiguous mounts. */
    getTransitionInfo(root: string, location: string, name: string, method: string): {
        location: string; name: string; method: string; path: string; serviceName: string | null;
        pathParameters: string[]; parameterMap: Record<string, { name: string; required: boolean }>;
    };
    isRenderModeValid(mode: string): boolean;
    isRenderModeText(mode: string): boolean;
    /** A screen-output mode such as xlsx, whose render is bytes. */
    isRenderModeBinary(mode: string): boolean;
    getMimeTypeByMode(mode: string): string | null;
    /** The server-static qvt report of a screen with the full check (an html render with real data), as the current user. */
    staticReport(options: { rootScreen: string; screenPath: string; parameters?: Record<string, unknown> }): {
        location: string; path: string; verdict: "clean" | "static half only" | "violations" | "stays a server merge";
        marked: boolean; eligible: boolean; fullCheck: boolean; reasons: string[];
        violations: { kind: string; element: string; detail: string }[] };
    render(options: MoquiScreenRenderOptions): { status: number; contentType: string; text: string; bytes: Uint8Array };
    makeRender(): MoquiScreenRenderBuilder;
    /** Java's ScreenTest: renders whose errors are collected instead of thrown. */
    makeTest(): MoquiScreenTest;
    /** The host call under `makeTest().render()`. */
    testRender(options: MoquiScreenRenderOptions & { method?: string; baseScreenPath?: string }): { output: string | null; errorMessages: string[]; renderTime: number; status: number | null };
}

interface MoquiEcContext {
    get(name: string): unknown;
    /** One map for the execution context, shared by every service call in it. Values are copied in and out. */
    readonly sharedMap: Record<string, unknown>;
    getSharedMap(): Record<string, unknown>;
}

interface MoquiNotificationMessage {
    topic(topic: string): MoquiNotificationMessage;
    subTopic(subTopic: string): MoquiNotificationMessage;
    userId(userId: string): MoquiNotificationMessage;
    userIds(userIds: string[] | Set<string>): MoquiNotificationMessage;
    userGroupId(userGroupId: string): MoquiNotificationMessage;
    message(message: MoquiMap | string): MoquiNotificationMessage;
    title(title: string): MoquiNotificationMessage;
    link(link: string): MoquiNotificationMessage;
    type(type: "info" | "success" | "warning" | "danger"): MoquiNotificationMessage;
    showAlert(show?: boolean): MoquiNotificationMessage;
    alertNoAutoHide(flag?: boolean): MoquiNotificationMessage;
    persistOnSend(flag?: boolean): MoquiNotificationMessage;
    emailTemplateId(id: string): MoquiNotificationMessage;
    emailMessageSave(flag?: boolean): MoquiNotificationMessage;
    send(persist?: boolean): MoquiNotificationMessage;
    getTopic(): string;
    /** The `NotificationTopic` value of the topic, or null. */
    getNotificationTopic(): EntityValue | null;
    /** Expanded by the last send, null before. */
    getTitle(): string | null;
    getLink(): string | null;
    getNotificationMessageId(): string | null;
    getNotifyUserIds(): string[];
}

/** The tools of the linked component crates that declare a type, by tool factory name. `tools.d.ts` adds to it. */
interface MoquiToolMap {}

/** A tool with no declared type: only the methods its Rust `Tool` exposes, taking and returning script values. */
interface MoquiTool {
    readonly [member: string]: (...args: unknown[]) => unknown;
}

/** `ec.factory.getToolFactory(name)`: a configured tool factory. */
interface MoquiToolFactory<T = MoquiTool> {
    readonly name: string;
    getInstance(...parameters: unknown[]): T;
}

/**
 * The server `ec` an action, a service script, or a script outside a render sees: the facade surface.
 * `ec.cache` and `ec.search` are outside the Server host methods list and are typed loosely.
 */
interface MoquiServerEc {
    /** The same as `ec.factory.getTool`. */
    getTool<K extends keyof MoquiToolMap>(name: K, ...parameters: unknown[]): MoquiToolMap[K];
    getTool(name: string, ...parameters: unknown[]): MoquiTool;
    readonly entity: MoquiEntityFacade;
    readonly service: {
        /** Expanded declarations, including auto-parameters; does not execute the service. */
        getServiceDefinition(name: string): (MoquiMap & {
            inParameterNames: string[];
            inParameterInfoMap: Record<string, MoquiParameterInfo>;
            outParameterInfoMap: Record<string, MoquiParameterInfo>;
            getInParameter(name: string): { attribute(name: string): string | null } | null;
        }) | null;
        readonly restApi: {
            /** Resource path without `/rest/s1`; metadata only, not an authorization check. */
            resolveService(path: string, method: string): {
                serviceName: string; path: string; method: string; pathParameters: string[];
            };
        };
        sync(): ServiceCallSync;
        async(): ServiceCallAsync;
        /** Epoch millis of the next time a Quartz cron expression (seconds first) is due after the moment, or null when never. */
        nextCronExecution(cron: string, after?: TimestampInput | null): number | null;
        /** English text; the locale and short format are not used. */
        getCronDescription(cron: string, locale?: unknown, shortFormat?: boolean): string;
        isLastRunBeforeLastSchedule(cron: string, lastRun?: TimestampInput | null, description?: string | null, now?: TimestampInput | null): boolean;
    };
    readonly resource: MoquiResourceFacade;
    readonly l10n: MoquiL10nFacade;
    readonly message: MoquiMessageFacade;
    readonly logger: MoquiLoggerFacade;
    readonly user: MoquiUserFacade;
    readonly web: MoquiWebFacade;
    readonly screen: MoquiScreenFacade;
    readonly context: MoquiEcContext;
    readonly factory: {
        readonly moquiVersion: string;
        /** Wait up to `retryLimit` (default 50) times 100 ms for async service work to finish; whether the pool is empty. */
        waitWorkerPoolEmpty(retryLimit?: number): boolean;
        /** Java's `getTool(name, parameters...)`: the object of a component's Rust tool factory. A tool that declares its type is in `MoquiToolMap` (`tools.d.ts`). */
        getTool<K extends keyof MoquiToolMap>(name: K, ...parameters: unknown[]): MoquiToolMap[K];
        getTool(name: string, ...parameters: unknown[]): MoquiTool;
        /** The tool factory a conf `tool-factory` element selected, or null when there is none of that name. */
        getToolFactory<K extends keyof MoquiToolMap>(name: K): MoquiToolFactory<MoquiToolMap[K]> | null;
        getToolFactory(name: string): MoquiToolFactory | null;
    };
    readonly cache: { getCache(name: string): { get(key: string): unknown; put(key: string, value: unknown): void; remove(key: string): boolean; containsKey(key: string): boolean; clear(): void } };
    /**
     * The tools calls (phase 11): cache admin, the entity and service model, screen statistics, the server status,
     * the SQL runner, the data writer and loader. They are outside the Server host methods list and typed loosely.
     */
    readonly artifactExecution: {
        isPermitted(name: string, artifactType?: string, action?: string): boolean;
        /** Entity ECA rules do not run for this execution context's writes. Returns whether they were already disabled. */
        disableEntityEca(): boolean;
        enableEntityEca(): void;
        entityEcaDisabled(): boolean;
        /** The artifact names on the stack, top first, joined with ", ". */
        getStackNameString(): string;
        /** Authorization is not checked for this execution context until `enableAuthz()`. Returns whether it was already disabled. */
        disableAuthz(): boolean;
        enableAuthz(): void;
        /** Whether authorization is disabled now, by a script or by the caller (a call with `disableAuthz()`, a job); changes nothing. */
        authzDisabled(): boolean;
    };
    /** A notification builder: `ec.makeNotificationMessage().topic("T").message({..}).userIds([..]).send()`. */
    makeNotificationMessage(): MoquiNotificationMessage;
    readonly search: { [method: string]: (...args: unknown[]) => unknown };
    /** The model calls of app code, in a build with the `llm` feature. */
    readonly llm: MoquiLlmFacade;
}

/** `ec.llm`: one completion from a configured model (`model`, else the binding of `feature`, else a model of `tier`). */
interface MoquiLlmFacade {
    /** With `schema` (a JSON schema) the reply is parsed and checked into `json`; a call that ends without an answer throws. */
    complete(options: {
        prompt?: string; system?: string; messages?: { role: "user" | "assistant"; text: string }[];
        model?: string; feature?: string; tier?: "fast" | "balanced" | "thorough";
        schema?: MoquiMap | string; schemaName?: string;
        reasoning?: "none" | "low" | "medium" | "high" | "max"; maxOutput?: number; timeoutSeconds?: number;
    }): {
        text: string; json: unknown; modelId: string; finish: string; inputTokens: number;
        cachedInputTokens: number; outputTokens: number; cost: number | null; callIds: string[];
    };
    isEnabled(): boolean;
}

// ---- server ec and sri during a screen render ----

/** `ec` while a server screen render is on the stack: the form-list keys of `ec.context` are set. */
interface MoquiRenderEc extends MoquiServerEc {
    readonly context: MoquiEcContext & { readonly formListFindId: string | null; readonly orderByField: string | null };
}

/** An element as generated screen code writes it. */
interface MoquiElement {
    name: string;
    attrs: Record<string, string>;
    text?: string;
    children?: MoquiElement[];
    parent?: MoquiElement | null;
}

interface MoquiScreenUrl {
    readonly path: string;
    readonly url: string;
    readonly pathWithParams: string;
    readonly readOnly: boolean;
    getPath(): string;
    getUrl(): string;
    getPathWithParams(): string;
    getUrlWithParams(extension?: string): string;
    getMinimalPathUrl(): string;
    isReadOnly(): boolean;
    isScreenUrl(): boolean;
    isTransition(): boolean;
    getTransitionMethod(): string;
    cloneUrlInstance(): MoquiScreenUrl;
    addParameter(name: string, value: unknown): MoquiScreenUrl;
    addParameters(parameters: MoquiMap): MoquiScreenUrl;
    removeParameter(name: string): MoquiScreenUrl;
    getParameterMap(): MoquiMap;
    getPassThroughParameterMap(): MoquiMap;
}

interface MoquiScreenDefInfo {
    readonly location: string;
    readonly screenName: string;
    getLocation(): string;
    getScreenName(): string;
    getDefaultMenuName(): string;
}

interface MoquiMenuItem {
    name: string;
    title: string;
    url: string;
    disabled: boolean;
    inPath: boolean;
    image: string | null;
    imageType: string | null;
    hasParameters: boolean;
}

interface MoquiFormListInfo {
    id: number;
    count: number;
}

/** `sri`: only while a server screen render is on the stack. Absent in actions that are not a render, and in the browser. */
interface MoquiScreenRenderer {
    readonly activeInCurrentMenu: boolean;
    write(text: string): void;
    getRenderMode(): string;
    doBoundaryComments(): boolean;
    encodeHtml(text: unknown): string;
    getActiveScreenDef(): MoquiScreenDefInfo;
    getScreenUrlInfo(): { targetScreen: MoquiScreenDefInfo; parentScreen: MoquiScreenDefInfo | null; extraPathNameList: string[] };
    sendRedirectAndStopRender(url: string): void;
    renderSection(name: string): string;
    renderSectionInclude(nameOrLocation: string): string;
    renderSubscreen(): string;
    renderIncludeScreen(location: string, shareScope?: string | boolean | null): string;
    renderText(location: string, template?: string | boolean | null): string;
    renderInlineText(body: string): string;
    getSectionIncludedNode(name: string): MoquiElement | null;
    once(key: string): boolean;
    pushContext(): void;
    popContext(): void;
    setInContext(element: MoquiElement): void;
    pushSingleFormMapContext(map: string | MoquiMap | null): void;
    getSingleFormMap(map: string | MoquiMap | null): MoquiMap;
    getFieldValue(field: MoquiElement, defaultValue?: string | null): unknown;
    getFieldValueString(widgetOrField: MoquiElement, defaultValue?: string | null, format?: string | null, type?: string | null): string;
    getFieldValuePlainString(field: MoquiElement, defaultValue?: string | null): string;
    getFieldValueClass(field: MoquiElement): string;
    getNamedValuePlain(name: string, form: MoquiElement): string;
    getFieldEntityValue(widget: MoquiElement): string;
    getFieldOptions(widget: MoquiElement): Map<string, string>;
    getFieldValidation(subNode: MoquiElement): { classes: string; regexp?: string; message?: string };
    getFormNode(name: string): MoquiElement;
    getFormInstance(name: string): { formNode: MoquiElement; name: string; list: boolean };
    getFormHiddenParameters(form: MoquiElement): MoquiMap;
    openFormList(name: string): MoquiFormListInfo;
    startFormListRow(info: MoquiFormListInfo, index: number): void;
    endFormListRow(): void;
    safeCloseList(info: MoquiFormListInfo): void;
    formListColumnCharWidths(...args: unknown[]): unknown;
    buildUrl(target: string): MoquiScreenUrl;
    buildUrlFromTarget(target: string): MoquiScreenUrl;
    makeUrlByType(origUrl: string, urlType: string, element?: MoquiElement | null, expandTransitionUrl?: string | boolean | null): MoquiScreenUrl;
    getScreenUrlInstance(): MoquiScreenUrl;
    getCurrentScreenUrl(): MoquiScreenUrl;
    isAnchorLink(element: MoquiElement, url: MoquiScreenUrl): boolean;
    getThemeIconClass(text: string): string | null;
    getThemeValues(resourceTypeEnumId: string): string[];
    getMenuItems(path?: string): MoquiMenuItem[];
    getMenuData(pathNames: string[]): MoquiMap[] | null;
    /** The `<shell-providers>` of the apps the user may open, `{apps: [{name, title, image, imageType, path, providers}]}`, only `kind` in each `providers` when given; kept per session. */
    getShellProviders(kind?: "quickCreate" | "todaySources" | "paletteActions" | "paletteTypes" | "paletteSources" | "notificationTopics" | "keymaps" | "accountMenuItems" | null): MoquiMap;
    appendToAfterScreenWriter(text: string): void;
    appendToScriptWriter(text: string): void;
    getAfterScreenWriterText(): string;
    getScriptWriterText(): string;
    staticUrl(text: string, urlType: string): { path: string; exists: boolean; isTransition: boolean; isScreen: boolean; readOnly: boolean; method: string };
    staticCheck(json: string): void;
    staticNote(kind: string, element: string, detail: string): void;
}

// ---- statics the generated script calls ----

/**
 * Bound once per runtime and returned by the bare-name proxy when the context has no such key. This port binds no
 * method on `ObjectUtilities` yet. `ScheduledJobRunner.getCronDescription` is not added (Server host methods, slice 2).
 */
interface MoquiObjectUtilities {}

interface MoquiCollectionUtilities {
    /** Sets `{list}Count`, `{list}PageIndex`, `{list}PageSize`, `{list}PageMaxIndex`, `{list}PageRangeLow`, `{list}PageRangeHigh` on `context`. */
    paginateParameters(count: number, listName: string, context: MoquiMap): MoquiMap;
    getMapArrayListValues(list: MoquiMap[], field: string, skipNull: boolean): unknown[];
}

interface MoquiStringUtilities {
    encodeForXmlAttribute(text: unknown, addZeroWidthSpaces?: boolean): string;
    base64UrlEncode(text: string): string;
    base64UrlDecode(text: string): string;
    /** Standard base64 of a text's UTF-8 bytes, or of a `Uint8Array` or `ArrayBuffer` as it is. */
    base64Encode(value: string | Uint8Array | ArrayBuffer | null): string;
    /** The bytes of standard base64 text; white space is skipped and the padding may be left out. Throws on other text. */
    base64Decode(text: string): Uint8Array;
}

interface MoquiWebUtilities {
    encodeHtml(text: unknown): string;
    encodeHtmlJsSafe(text: unknown): string;
    fieldValuesEncodeHtmlJsSafe(values: unknown): string;
    ipMatches(patterns: string, address: string): boolean;
    isSafeSinglePathSegment(segment: string): boolean;
    isSameOriginRedirect(url: string, host?: string | null): boolean;
    lastPathSegment(path: string): string;
}

// ---- the unwrapped projection ----

/**
 * `ContextJavaUtil.unwrapMap`: plain maps, a decimal as a string, a timestamp as ISO-8601 text, an entity value as its
 * field map, an entity list as a list of those maps. A find, `ec`, `sri` and functions are dropped.
 * This is the JSON, the locked-down template, and the browser shape.
 */
type MoquiUnwrap<T> = T extends Decimal
    ? string
    : T extends Timestamp
      ? Timestamp
      : T extends string | number | boolean | null | undefined
        ? T
        : T extends EntityValue<infer N>
          ? MoquiPlain<N>
          : T extends EntityList<infer N>
            ? MoquiPlain<N>[]
            : T extends EntityFind<string> | MoquiServerEc | MoquiScreenRenderer | ((...args: never[]) => unknown)
              ? never
              : T extends readonly (infer U)[]
                ? MoquiUnwrap<U>[]
                : T extends object
                  ? MoquiUnwrapMap<T>
                  : unknown;
type MoquiUnwrapMap<T> = {
    [K in keyof T as [MoquiUnwrap<T[K]>] extends [never] ? never : K]: MoquiUnwrap<T[K]>;
};
/** An entity value as plain data: its field map. */
type MoquiPlain<N extends string> = { [K in MoquiFieldName<N>]?: MoquiUnwrap<MoquiFields<N>[K]> | null };

// ---- screens ----

/** Filled by the generated `screens.d.ts`. */
interface MoquiScreenContexts {}
interface MoquiScreenPackages {}
interface MoquiFormListPackages {}

// ---- the browser host and the server-static qvt package ----

/** The restricted `ec` of a server-static qvt screen in the browser. Any other member throws there. */
interface MoquiBrowserEc {
    readonly l10n: {
        localize(text: string): string;
        format(value: unknown, pattern?: string | null): string;
        formatCurrency(amount: unknown, uomId?: string | null, fractionDigits?: number | null): string;
        formatCurrencyNoSymbol(amount: unknown, uomId?: string | null, fractionDigits?: number | null): string;
    };
    readonly user: {
        readonly userId: string | null;
        readonly username: string | null;
        readonly locale: string;
        readonly timeZone: string;
        readonly userAccount: {
            partyId?: string | null;
            emailAddress?: string | null;
            userFullName?: string | null;
            locale?: string | null;
            timeZone?: string | null;
        } | null;
        getPreference(key: string): string | null;
    };
    readonly resource: {
        /** The template carries a table of functions by expression text; the browser has no eval. */
        expand(text: string, table?: Record<string, () => unknown>): string;
        expandNoL10n(text: string, table?: Record<string, () => unknown>): string;
    };
}

/** A drop-down option the server resolved: `mqOpt_{form}_{field}_{sub}_{widget}`. */
interface MoquiOption {
    value: string;
    label: string;
}

type MoquiPageKey<L extends string> = `${L}${MoquiPageSuffix}`;
/** The plain projection with credential names removed at any depth, as `stripCredentialParameters` removes them. */
type MoquiStripped<T> = T extends readonly (infer U)[]
    ? MoquiStripped<U>[]
    : T extends string | number | boolean | null | undefined
      ? T
      : T extends object
        ? { [K in keyof T as K extends string ? (Lowercase<K> extends MoquiCredentialName ? never : K) : K]: MoquiStripped<T[K]> }
        : T;

/**
 * `GET {screen}/actions`: the context after data prep, unwrapped, credentials stripped, the lists and page keys of every
 * form-list removed (`L`), plus the `mqExtra_{form}` maps of the form-singles (`X`). Keys the generator cannot prove are unknown.
 */
type MoquiScreenPackage<C, L extends string, X> = MoquiStripped<
    MoquiUnwrapMap<{ [K in keyof C as K extends L | MoquiPageKey<L> ? never : K]: C[K] }>
> &
    X & { [name: string]: unknown };

/** `GET {screen}/actions/{formName}`: the rows, the page keys, the header values, the option lists. */
type MoquiFormListPackage<L extends string, R, X> = { [K in L]: MoquiStripped<R>[] } & {
    [K in MoquiPageKey<L>]?: number;
} & {
    orderByField?: string;
    pageIndex?: number | string;
    pageSize?: number | string;
    formListFindId?: string;
} & X;

/** The scope a server-static template expression sees: the package and the browser `ec`. */
type MoquiStaticScope<P> = P & { readonly ec: MoquiBrowserEc };
