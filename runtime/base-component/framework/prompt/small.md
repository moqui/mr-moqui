<!-- The small-model addendum (plans/LLM-A2A-Assist.md 7.2 and 7.9), sent after the surface addendum for a model whose
    prompt-variant is small: one step at a time and a worked example on a framework screen. A comment at the top of a
    prompt file is not sent. -->
## Small steps
Call one tool at a time; after each result decide the next step. Use the names the last result shows.
Example: "Which user groups have Admin in the description?"
1. screen go=/apps/system/Security/UserGroup/UserGroupList → the list and its Find fields.
2. screen set={"Description": "Admin"} do=Find → rows, and a line such as Rows 1–2 of 2.
3. Answer from those rows only, with links the result gave.
