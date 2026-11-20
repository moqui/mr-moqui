// The sealed start with the tpm method across a real reboot (plans/Sealed-Secrets.md §3.3, §4 S3; docs/Run-Deploy.md,
// Sealed start): an acceptance test, since it needs a Linux host with a TPM2 and systemd-creds, MR Moqui installed
// under systemd/moqui.service, and a reboot. The regression suite covers the same rules on one machine with a stub
// systemd-creds (crates/moqui-host/tests/it/sealed_start.rs).
//
// The target is the server's URL; the host's own sshd runs the operator commands. Two runs, one each side of the
// reboot the first one starts:
//
//   moqui test-client framework --acceptance --spec SealedStart https://erp.example.com:8443 \
//     --param phase=before --param hostSsh=erp.example.com:22 --param hostUser=admin \
//     --param hostFingerprint=SHA256:... (and MOQUI_TEST_HOST_KEY, the user's private key, PEM)
//   (the host reboots)
//   moqui test-client ... --param phase=after ... --param rootId=<the root_id the before run logged>
//
// hostUser runs `sudo -n /opt/moqui/moqui operator ...` and `sudo -n systemctl ...` with no password. Without hostSsh
// every feature skips, so the framework's acceptance run on this machine passes over it.

const phase = params.phase ?? "before"
const operator = `sudo -n ${params.moquiBin ?? "/opt/moqui/moqui"} operator --socket ${params.socket ?? "/run/moqui/moqui.sock"}`

const host = () => {
    if (!params.hostSsh) skip("no host to reboot: --param hostSsh=host:port (and hostUser, MOQUI_TEST_HOST_KEY)")
    const [address, port] = params.hostSsh.split(":")
    return ssh.connect({
        host: address,
        port: Number(port ?? 22),
        user: params.hostUser,
        key: params.hostKey,
        fingerprint: params.hostFingerprint,
        timeout: 30,
    })
}

// `moqui operator status` as its lines, by their label
const status = (conn) => {
    const run = conn.exec(`${operator} status`, { timeout: 30 })
    const lines = {}
    for (const line of run.stdout.split("\n")) {
        const at = line.indexOf(": ")
        if (at > 0) lines[line.slice(0, at)] = line.slice(at + 2)
    }
    return { run, lines }
}

feature("before the reboot: the server is unsealed by the tpm method and serves", {
    when() {
        if (phase !== "before") skip(`phase is ${phase}`)
        const conn = host()
        return { conn, seal: status(conn), login: http.get("/Login") }
    },
    then({ conn, seal, login }) {
        seal.run.status == 0
        seal.lines.State == "unsealed, method tpm"
        login.status == 200
        // the root id the after run compares, from seal.json (readable by root only)
        const file = conn.exec("sudo -n cat /opt/moqui/db/seal.json", { timeout: 30 })
        file.status == 0
        const sealJson = JSON.parse(file.stdout)
        sealJson.method.kind == "tpm"
        sealJson.recovery.threshold >= 2
        log.info(`rootId=${sealJson.root_id}: pass it to the after run`)
        // the unit as Sealed-Secrets §3.5 and §3.3 have it
        const unit = conn.exec("systemctl show moqui -p LimitCORE -p RuntimeDirectory -p Type", { timeout: 30 })
        unit.stdout.includes("LimitCORE=0")
        unit.stdout.includes("RuntimeDirectory=moqui")
        unit.stdout.includes("Type=notify")
        // the reboot: the connection drops with it
        conn.exec("sudo -n systemctl reboot", { timeout: 10, expect: ["timeout"] })
    },
})

feature("after the reboot: the TPM unsealed the same root key with no operator, then the listeners opened", {
    when() {
        if (phase !== "after") skip(`phase is ${phase}`)
        // the host comes back and the server serves, with nobody at the socket
        waitUntil(() => {
            try {
                return session().get("/Login").status == 200
            } catch (err) {
                return false
            }
        }, { timeout: Number(params.rebootSeconds ?? 600), interval: 5000 })
        const conn = host()
        return { conn, seal: status(conn) }
    },
    then({ conn, seal }) {
        seal.lines.State == "unsealed, method tpm"
        seal.lines.Reason == null
        const file = JSON.parse(conn.exec("sudo -n cat /opt/moqui/db/seal.json", { timeout: 30 }).stdout)
        params.rootId == null || file.root_id == params.rootId
        file.generation >= 1
        // systemd heard the unsealed status
        conn.exec("systemctl show moqui -p StatusText", { timeout: 30 }).stdout.includes("StatusText=unsealed")
        // the operator socket refuses the service user anything but status
        const asService = conn.exec(`sudo -n -u moqui ${params.moquiBin ?? "/opt/moqui/moqui"} operator --socket /run/moqui/moqui.sock seal`, { timeout: 30 })
        asService.status != 0
        asService.stderr.includes("only status") || asService.stdout.includes("only status")
    },
})
