/* The site component's specs: ./moq test @component@ runs them. */
spec({}, () => {
    feature("the site component loads", {
        expect: () => {
            ec.factory.getComponentInfoList().some((component) => component.name === "@component@")
        }
    })
})
