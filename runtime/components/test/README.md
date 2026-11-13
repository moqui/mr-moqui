# The test set

Add-on components for testing that never go to production: cross-component regression and acceptance fixtures,
scenarios that span several components, pre-production validation. `components.xml` loads this set with
`purpose="dev, test"`: on a dev instance (`./moq run`, `./moq test`) and on a test instance (staging), never on
production. `./moq archive` leaves it out unless `--with-tests`. Put a component here with
`./moq component-get --test <name>` or `./moq component-create --test <name>`; each is a repository of its own.

A component's own tests stay in its `test/` directory ([the two suites](../../../docs/Build-Test.md#the-two-suites));
this set is for components that are only tests.
