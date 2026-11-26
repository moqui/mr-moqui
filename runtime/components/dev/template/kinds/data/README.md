# @component@

@Title@, an MR Moqui component of data only: no screens, services or code.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name and version, and `depends-on` for the components whose entities its data fills. |
| `data/10-seed/@Class@SeedData.xml` | Seed (`type="seed"`): configuration every instance needs, here an enumeration type and its values. |
| `data/30-demo/@Class@DemoData.xml` | Demo (`type="demo"`): records for a dev or demo instance, here a value of the seed's type. |
| `test/@Class@Data.spec.js` | The regression suite: the seed records are there. |

A data file's `type` decides which loads read it: `./moq load` with no types reads every type, a production load and
`moqui load` with none read `seed`, `seed-initial` and `install`, and a load names the types it wants
(`./moq load seed demo`). A load reads a directory's files in name order and then its subdirectories in name order, so
the numbered directories put the seed records before the demo records that refer to them.

## Run and test

- `./moq load` loads its data with the rest; `./moq load -l <path of a data file>` loads one file.
- `./moq test @component@` runs its regression specs.

Data files: `docs/dev/Practices-App-Dev.md` in mr-moqui.
