// The gallery sweep in the /qapps shell (./moq accept example), against a server that has loaded the example component
// (and its demo data). An empty specs list runs the example's acceptance specs (the Gallery sweep) before the server
// listens.
suite(["GallerySmoke"], { server: { components: ["example"] } })
