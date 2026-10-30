// Gallery shots against a server that has loaded the example component (and its demo data). An empty specs list
// runs every example spec before the server listens; those specs skip when a feature they need is not built.
suite(["GalleryShots"], { server: { components: ["example"] } })
