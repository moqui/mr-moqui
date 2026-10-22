---
name: create-user-account
title: Creating a user account
description: Use when the user asks to add a login, a user or a user account for someone.
kind: procedure
aliases: [login, new user, add user, account]
---
A user account is made on System › Security › User Accounts, whose Create User Account dialog runs the service that
hashes the password. Never make one by a generated view, and never write a password into chat, a view or a tool call.

## Steps
1. `search` for the username or the email first. One hit: that account exists; open it instead. No hit: go on.
2. `screen go=/apps/system/Security/UserAccount/UserAccountList`, then `do` Create User Account.
3. `set` Username, Email and Full Name with the values the user gave. Leave Password and Password Verify empty.
4. Do not press Create. Tell the user the dialog is filled and that they type the password and press Create
   themselves; a password is theirs to choose.

## After
To add the account to a user group, open its detail screen (the list's row) and use its groups; each save there asks
for the user's approval.
