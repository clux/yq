# lq - low overhead yq/tq/... implementation
[![CI](https://github.com/clux/lq/actions/workflows/release.yml/badge.svg)](https://github.com/clux/lq/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/lq.svg)](https://crates.io/crates/lq)
[![dependency status](https://deps.rs/repo/github/clux/lq/status.svg)](https://deps.rs/repo/github/clux/lq)

A lightweight and portable [jq](https://jqlang.github.io/jq/) wrapper for doing arbitrary queries from **YAML**/**TOML**/**JSON** documents by converting to **JSON** and passing to `jq`, then returning the result either as raw `jq` output, or back into TOML or YAML.

## Installation

Via cargo:

```sh
cargo install lq
```

or download a prebuilt from [releases](https://github.com/clux/lq/releases) either manually, or via [binstall](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall lq
```

**Note**: Depends on `jq` being installed.

### lq as yq

Because yaml is the default input language, you can use it as your top level `yq` executable with a symlink or alias:

```sh
# globally make yq be lq
ln -s $(which lq) /usr/local/bin/yq

# alias yq to lq in shell environment only
alias yq=lq
```

This should be compatible with `python-yq`, but it has some differences with the go yq.

## Why / Why Not

### jq compatibility

- arbitrary `jq` usage on any input format (yaml/toml/json)
- [same filter syntax](https://jqlang.github.io/jq/manual/#basic-filters) (shells out to `jq`)
- matches `jq`'s cli interface (only some extra input/output format controlling flags)
- supports `jq` output formatters such as `-c`, `-r`, and `-j` (compact, raw, joined output resp)
- supports [jq modules](https://jqlang.github.io/jq/manual/#modules) on all input formats

### Features

- reads __multidoc yaml__ input, handles [yaml merge keys](https://yaml.org/type/merge.html) (expanding tags)
- splits __multidoc__ input by keys/fns into multiple files
- reads from __stdin xor file__ (file if last arg is a file)
- output conversion shortcuts: `-y` (YAML) or `-t` (TOML)
- drop-in replacement to [python-yq](https://kislyuk.github.io/yq/) (with `alias yq=lq`)
- ~[1MB](https://github.com/clux/lq/releases/latest) in binary size (for small cloud CI images / [binstalled ci actions](https://github.com/cargo-bins/cargo-binstall#faq))

### Limitations

- Shells out to `jq` (supports what your `jq` version supports)
- Expands [YAML tags](https://yaml.org/spec/1.2-old/spec.html#id2764295) (input is [singleton mapped](https://docs.rs/serde_yaml/latest/serde_yaml/with/singleton_map/index.html) -> [recursively](https://docs.rs/serde_yaml/latest/serde_yaml/with/singleton_map_recursive/index.html), then [merged](https://docs.rs/serde_yaml/latest/serde_yaml/value/enum.Value.html#method.apply_merge)) - so tags are [not preserved](https://github.com/clux/lq/issues/12) in the output
- Does not preserve indentation (unsupported in [serde_yaml](https://github.com/dtolnay/serde-yaml/issues/337))
- Does not support [duplicate keys](https://github.com/clux/lq/issues/14) in the input document
- No XML/CSV support (or other more exotic formats)

## Usage

### YAML Input

Use as [jq](https://jqlang.github.io/jq/tutorial/) either via stdin:

```sh
$ lq '.[3].kind' -r < test/deploy.yaml
Service

$ lq -y '.[3].metadata' < test/deploy.yaml
labels:
  app: controller
name: controller
namespace: default
```

or from a file arg (at the end):

```sh
$ lq '.[3].kind' -r test/deploy.yaml
$ lq -y '.[3].metadata' test/deploy.yaml
```

The default input format is YAML and is what the binary is named for (and the most common primary usage case).


### TOML Input

Using say `Cargo.toml` from this repo as input, and aliasing `tq='lq --input=toml'`:

```sh
$ tq '.package.categories[]' -r < Cargo.toml
command-line-utilities
parsing

$ tq -t '.package.metadata' < Cargo.toml
[binstall]
bin-dir = "yq-{ target }/{ bin }{ format }"
pkg-url = "{ repo }/releases/download/{ version }/yq-{ target }{ archive-suffix }"

$ tq -y '.dependencies.clap' < Cargo.toml
features:
- cargo
- derive
version: 4.4.2

$ tq '.profile' -c < Cargo.toml
{"release":{"lto":true,"panic":"abort","strip":"symbols"}}
```

Add `alias tq='lq --input=toml'` to your `.bashrc` or `.zshrc` (etc) to make this permanent if you find it useful.

### JSON Input

If you need to convert json to another format you pass `--input=json`:

```sh
$ lq --input=json '.ingredients | keys' -y < test/guacamole.json
- avocado
- coriander
- cumin
- garlic
- lime
- onions
- pepper
- salt
- tomatoes
```

### Advanced Examples
Select with nested query and raw output:

```sh
$ lq '.spec.template.spec.containers[].image' -r < test/grafana.yaml
quay.io/kiwigrid/k8s-sidecar:1.24.6
quay.io/kiwigrid/k8s-sidecar:1.24.6
docker.io/grafana/grafana:10.1.0
```

Select on multidoc:

```sh
$ lq -y '.[] | select(.kind == "Deployment") | .spec.template.spec.containers[0].ports[0].containerPort' test/deploy.yaml
8000
```

Escaping keys with slashes etc in them:

```sh
lq -y '.updates[] | select(.["package-ecosystem"] == "cargo") | .groups' .github/dependabot.yml
```

Using helpers from `jq` [modules](https://jqlang.github.io/jq/manual/#modules) e.g. [k.jq](https://github.com/clux/lq/blob/main/test/modules/k.jq):

```sh
$ lq 'include "k"; .[] | gvk' -r -L$PWD/test/modules < test/deploy.yaml
v1.ServiceAccount
rbac.authorization.k8s.io/v1.ClusterRole
rbac.authorization.k8s.io/v1.ClusterRoleBinding
v1.Service
apps/v1.Deployment
```

### Output Caveats

Output formatting such as `-y` for YAML or `-t` for TOML will require the output from `jq` to be parseable json.
If you pass on `-r`,`-c` or `-c` for raw/compact output, then this will generally not be parseable as json.

### Debug Logs

The project respects `RUST_LOG` when set, and sends these diagnostic logs to stderr:

```sh
$ RUST_LOG=debug lq '.version' test/circle.yml
2023-09-18T23:17:04.533055Z DEBUG lq: args: Args { input: Yaml, output: Jq, yaml_output: false, toml_output: false, in_place: false, jq_query: ".version", file: Some("test/circle.yml"), compact_output: false, raw_output: false, join_output: false, modules: None }
2023-09-18T23:17:04.533531Z DEBUG lq: found 1 documents
2023-09-18T23:17:04.533563Z DEBUG lq: input decoded as json: {"definitions":{"filters":{"on_every_commit":{"tags":{"only":"/.*/"}},"on_tag":{"branches":{"ignore":"/.*/"},"tags":{"only":"/v[0-9]+(\\.[0-9]+)*/"}}},"steps":[{"step":{"command":"chmod a+w . && cargo build --release","name":"Build binary"}},{"step":{"command":"rustc --version; cargo --version; rustup --version","name":"Version information"}}]},"jobs":{"build":{"docker":[{"image":"clux/muslrust:stable"}],"environment":{"IMAGE_NAME":"lq"},"resource_class":"xlarge","steps":["checkout",{"run":{"command":"rustc --version; cargo --version; rustup --version","name":"Version information"}},{"run":{"command":"chmod a+w . && cargo build --release","name":"Build binary"}},{"run":"echo versions"}]},"release":{"docker":[{"image":"clux/muslrust:stable"}],"resource_class":"xlarge","steps":["checkout",{"run":{"command":"rustc --version; cargo --version; rustup --version","name":"Version information"}},{"run":{"command":"chmod a+w . && cargo build --release","name":"Build binary"}},{"upload":{"arch":"x86_64-unknown-linux-musl","binary_name":"${IMAGE_NAME}","source":"target/x86_64-unknown-linux-musl/release/${IMAGE_NAME}","version":"${CIRCLE_TAG}"}}]}},"version":2.1,"workflows":{"my_flow":{"jobs":[{"build":{"filters":{"tags":{"only":"/.*/"}}}},{"release":{"filters":{"branches":{"ignore":"/.*/"},"tags":{"only":"/v[0-9]+(\\.[0-9]+)*/"}}}}]},"version":2}}
2023-09-18T23:17:04.533650Z DEBUG lq: jq args: [".version"]
2023-09-18T23:17:04.538606Z DEBUG lq: jq stdout: 2.1

2.1
```
