# @component@

@Title@, a skill pack: skills and prompts for the agents, and no code.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name and version. |
| `skill/@package@.md` | A procedure skill: front matter (`name`, `description` starting "Use when", `kind`, `aliases`) and its steps. |
| `data/@Class@PromptData.xml` | Seed: a suggested prompt of the Assist drawer (`AgentSuggestedPrompt`). |
| `test/skill-retrieval.json` | The retrieval cases: the messages users write and the skill each must find. |
| `test/@Class@Skills.spec.js` | The regression suite: the skill is in the index from its file, and the prompt is seeded. |

Each Markdown file under `skill/` is a skill: the catalog lists its description, and an agent loads its text by name
when it needs it. A skill's front matter can name the agents it is for (`agents: [dev]`) and the screens of a procedure
(`screens:`). How to write one: the framework's `skill-authoring` skill
(`runtime/components/base/framework/skill/skill-authoring.md`).

## Run and test

- The skills are read when the component loads, and again when a skill file is saved; `./moq load` loads the prompts.
- `./moq test @component@` runs its regression specs; `./moq test @component@ --skills` runs the retrieval cases with
  no model.
