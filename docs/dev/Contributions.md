# Contributions

How code from a pull request gets into an mr-* repository. Every repository has an `AUTHORS.md` with a Copyright
Waiver and a Grant of Patent License, and each has a list of signatures. Its Contributing section is the policy for
contributors: we accept code only from people who have signed, and from an account that has not signed we may take
the ideas but not the code. This page is for maintainers, and for the agents working for them.

## Is it signed?

Check every pull request, in the repository it is for (each repository has its own `AUTHORS.md`):

```sh
gh pr view <n> --json author,commits --jq '[.author.login] + [.commits[].authors[].login] | unique'
git log --follow --format='%h %an <%ae> %ad %s' --date=short -S'<login>' -- AUTHORS.md
```

- It is signed when the pull request's author and each author of its commits have a line in both lists of
  `AUTHORS.md` (under the Copyright Waiver and under the Grant of Patent License).
- Each signature must come from a commit by that account. The `git log -S` shows which commit added it, and who made
  that commit (`--follow` reaches back past a rename from `AUTHORS`).
- A pull request that carries another person's commits needs that person's signature too.
- A signature can be the first commit of the pull request itself. A pull request with only the signature is easiest
  to check and merge.

## Signed

Review and test it as the [Issue and Pull Request Guide](https://moqui.org/m/docs/moqui/Issue+and+Pull+Request+Guide)
says. Merge so that a signature commit keeps its author: a merge commit or a rebase, not squashed into another
person's work.

## Unsigned

Ask the author to sign, with a link to the Contributing section of `AUTHORS.md`. When they add the signature commit,
go on as for a signed pull request. A signature added later covers their earlier commits, since the statements cover
"any and all copyright interest in this software".

If they do not sign, close the pull request without merging or cherry-picking any of it. When the idea is wanted, write
the change again:

- Start from a description of what the change does and why: the pull request's text, or an issue written from it.
- Do not start from its diff, and do not give the diff to an AI tool to adapt. Taking the idea and not the code
  depends on the new change being written from the idea.
- Credit the idea in the new commit's message, by the pull request's number (`from #123`).

## Bots and AI tools

AI tools do not sign: the person who uses them and submits the work signs, and is responsible for it (the
[AI/LLM Policy](../../README.md#aillm-policy)). A pull request opened by a bot or app account counts as signed only
when the person who runs it has signed and says so on the pull request. Otherwise it is unsigned.

## Contributors from before AUTHORS.md

People who committed before a repository had an `AUTHORS.md` are asked to sign it, each with a pull request that has
only the signature commit, in each repository they committed to.
