// The Offline maps package transition as the apps' download asks it (plans/Maps-Offline.md §2.3; moqui-client
// packages): a GET with packageId and revision in the URL. A transition that writes is refused with its parameters in the
// URL (400), so it is read-only; found by the emulator walk (scripts/android-maps-offline.sh), since the server spec
// MapsPackages renders it in process, where the URL rule does not apply. A package that is not there is 404.
feature("the package transition takes its packageId and revision in the URL, as the apps ask it", {
    expect() {
        const s = session({user: "john.doe"})
        const res = s.get("/apps/maps/Offline/package", {
            params: {packageId: "NO_SUCH_PACKAGE", revision: "1"},
            headers: {Range: "bytes=0-99"}
        })
        res.status == 404
        String(res.text).includes("Offline map not available")
    }
})
