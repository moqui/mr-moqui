# Dev instances with a dev group server

Each developer runs a dev instance on their own machine. The team shares a group server, a production instance with
IronStone BOSS that hosts the components' repositories, the Projects work items and the LLM gateway. Developers who
do not run a server work on a shared dev-aide server, a dev instance that uses the group server as a laptop does.
Steps and what is not shared: [Run-Deploy](../../../docs/Run-Deploy.md#dev-instances-with-a-dev-group-server).

```
 laptop      ./moq run, dev, 127.0.0.1 ---+   git over https or ssh, /llm, /rest/s1/mantle/my/tasks
 laptop      ./moq run, dev, 127.0.0.1 ---+--------> group server team.example.com, production purpose
 shared dev-aide server, dev, https ------+          MoquiGroupConf.xml: mr-boss, mr-mantle
   dev_aide_shared=true                              one server or a cluster (../local-cluster/)
```

| Node | Conf | Package and runtime |
|---|---|---|
| Group server | `MoquiGroupConf.xml` | Maximal, or Enterprise as a cluster; mr-boss, which brings mr-mantle |
| Dev instance | `../MoquiDevConf.xml`, the dev conf, with the settings below in its `.env` | Dev, the default |
| Shared dev-aide server | `../MoquiDevConf.xml`, with the settings below in its `.env` | Dev |

The dev instances keep the dev conf, which is the whole of a dev instance's settings; what this topology adds to it
is a few properties.

## `.env`

- **Group server**: `webapp_http_host` and HTTPS as for one production server; the model endpoints' keys (such as
  `llm_openai_api_key`); `ssh_public_port` when clients reach SSH on another port.
- **Dev instance**:

  ```sh
  webapp_http_bind=127.0.0.1                     # Dev AIDE gives its users code execution
  llm_openai_url=https://team.example.com/llm    # no /v1: the endpoint adds /v1/chat/completions
  llm_openai_api_key=<access token>              # from the group server's Projects › Code › Git access
  llm_openai_model=<alias>
  ```

- **Shared dev-aide server**: `dev_aide_shared=true`, and `webapp_allow_origins` set to its own origin, such as
  `https://dev.example.com`, in place of the dev conf's `*`; a certificate in `cert/`, `webapp_http_bind` empty.

## Commands

```sh
# the group server
./moq component-get mr-boss
./moq run-production --release -c conf/dev-group/MoquiGroupConf.xml

# a dev instance, with the team's components in myaddons.xml from the group server's repositories
./moq component-get acme-site
./moq run
```
