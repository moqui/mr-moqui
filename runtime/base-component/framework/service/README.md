# Framework services

Service definitions (`*.xml`) and their script bodies (`*.js`) under `org/moqui/`. A file's directory is its package,
so `org/moqui/chat/ChatServices.xml` holds `org.moqui.chat.ChatServices.*`. SECAs and other ECA files are at the top.

| Package | What is in it |
|---|---|
| (top level) | `org.moqui.EmailServices`, `EntityServices`, `GitServices`, `ImageServices`, `SmsServices` |
| `impl` | Core plumbing from Moqui Java: basic lookups, entity, entity sync, image, print, screen, server, service jobs, system messages, users |
| `admin` | Access, identity and constitution administration |
| `agent` | Agents, AI tasks, assist |
| `call` | Calls and voice: call handling, queues, recordings, captions, transfers, IVR, SIP devices, phone texts, speech, voicemail |
| `chat` | Chat rooms, bridges, chat keys, XMPP |
| `dev` | Developer and operator tools: dev tools, conf reload, database tools, ETL, sim runs, instances |
| `edge` | Edge node links and client sync |
| `ext` | Vendor and external integrations (Google) |
| `git` | Git repositories and merge requests |
| `mail` | Sending email and the mail client |
| `monitor` | Monitoring and telemetry |
| `public` | Public account services |
| `report`, `search`, `sftp` | Reports, search indexing, SFTP messages |
| `resource` | Files and drives, DAV, content blobs, spreadsheets, collaborative documents, elFinder, wiki |
| `sso` | Sign-in and recent authentication |
| `ui` | Shell counts and providers, status colors, notifications, units |

Put a new service file in the package for its area, not in `impl`.
