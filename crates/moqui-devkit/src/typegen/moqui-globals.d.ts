// The names a server script sees without declaring them (plans/ContextObjects.md). Declarations only.
// A bare name on the context stack is not declared here: read it through `context`, or declare it in the script.

/** The execution context: the facade surface. During a screen render it is also `MoquiRenderEc`. */
declare const ec: MoquiServerEc;
/** The context stack. A screen script can narrow it: `context as MoquiScreenContexts["component://..."]`. */
declare const context: MoquiContext;
/** The result map of a service's actions. */
declare let result: MoquiMap;
/** The screen renderer. Its methods work only while a server screen render is on the stack. */
declare const sri: MoquiScreenRenderer;

declare const ObjectUtilities: MoquiObjectUtilities;
declare const CollectionUtilities: MoquiCollectionUtilities;
declare const StringUtilities: MoquiStringUtilities;
declare const WebUtilities: MoquiWebUtilities;
declare const System: { getProperty(name: string): string | null };

/** A directory entry of `SftpClient.ls`. Times are seconds since the epoch. */
interface SftpFileInfo {
    name: string;
    path: string;
    isFile: boolean;
    size: number;
    lastAccess: number;
    lastModified: number;
}
interface SftpReadStream {
    read(max?: number): Uint8Array;
    readAllBytes(): Uint8Array;
    readText(charset?: string | null): string;
    close(): void;
}
interface SftpWriteStream {
    write(data: string | Uint8Array, charset?: string | null): void;
    close(): void;
}
/** The `moqui-sftp` client (`org.moqui.sftp.SftpClient`). Without a port in `host` the port is 115, as in Java. */
declare class SftpClient {
    static readonly SFTP_PORT: 115;
    static readonly SSH_PORT: 22;
    constructor(host: string, username: string, port?: number | null);
    password(password: string): this;
    publicKeyPkcs8(privateKeyPem: string, publicKey?: string | null): this;
    preserveAttributes(preserve: boolean): this;
    /** Pin the server's host key, `SHA256:...`; without it the key is trusted on first use. */
    hostKeyFingerprint(fingerprint: string): this;
    connect(): this;
    isConnected(): boolean;
    lsFiles(path: string): string[];
    ls(path: string): SftpFileInfo[];
    getBinary(path: string): Uint8Array;
    getText(path: string, charset?: string | null): string;
    put(path: string, filename: string, data: string | Uint8Array | MoquiUploadedFile | { getLocation(): string },
        charset?: string | null, createDir?: boolean): this;
    openStream(path: string): SftpReadStream;
    putStream(path: string): SftpWriteStream;
    moveFile(filePath: string, destDirPath: string, createDir?: boolean): string;
    renameFile(filePath: string, newFileName: string): string;
    rename(filePath: string, newFilePath: string): string;
    rm(path: string): this;
    close(): void;
}

/** The compare of `<if-compare>` and `<econdition>`: `operator` is a Moqui operator name such as `equals`. */
declare function compare(
    field: unknown,
    operator: string,
    value: unknown,
    toField?: unknown,
    format?: string | null,
    type?: string | null,
): boolean;
/** `ObjectUtilities.basicConvert`: `type` is a Java type name such as `BigDecimal`. */
declare function basicConvert(value: unknown, type: string): unknown;
/** A decimal from a decimal, a number or numeric text; anything else throws. */
declare function toDecimal(value: Decimal | number | string): Decimal;
declare function orderMapList(list: unknown[] | null, fields: string[]): void;
declare function filterMapList(list: unknown[] | null, values: MoquiMap): void;
declare function filterMapListByDate(
    list: unknown[] | null,
    fromField: string,
    thruField: string,
    moment?: TimestampInput | null,
    ignoreIfEmpty?: boolean,
): void;
