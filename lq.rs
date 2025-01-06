use anyhow::Result;
use clap::{Parser, ValueEnum};
use serde_yaml::{self, with::singleton_map_recursive, Deserializer};
use std::io::{stderr, stdin, BufReader, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing::*;

#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Input {
    #[default]
    Yaml,
    Json,
    Toml,
}

#[derive(Copy, Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Output {
    Yaml,
    #[default]
    Jq,
    Toml,
}

/// A lightweight and portable Rust implementation of a common jq wrapper
///
/// Allows doing arbitrary jq style queries editing on YAML documents.
///
/// yq '.[3].kind' < .github/dependabot.yaml
///
/// yq -y '.updates[0].schedule' .github/dependabot.yml
///
/// yq '.spec.template.spec.containers[].image' -r
///
/// yq '.[].kind' -r < manifest.yml
///
/// yq -y '.[2].metadata' < manifest.yml
#[derive(Parser, Debug, Default)]
#[command(author, version, about)]
struct Args {
    /// Input format of the input file or stdin
    #[arg(long, value_enum, default_value_t)]
    input: Input,
    /// Output format to convert the jq output into
    #[arg(long, value_enum, default_value_t)]
    output: Output,

    /// Convert jq output to YAML (shortcut for --output=yaml)
    #[arg(
        short = 'y',
        long,
        default_value = "false",
        conflicts_with = "toml_output",
        conflicts_with = "output"
    )]
    yaml_output: bool,
    /// Convert jq output to TOML (shortcut for --output=toml)
    #[arg(
        short = 't',
        long,
        default_value = "false",
        conflicts_with = "yaml_output",
        conflicts_with = "output"
    )]
    toml_output: bool,

    /// Edit the input file in place
    #[arg(short, long, default_value = "false")]
    in_place: bool,

    /// Split the output by keys and serialize into a folder
    ///
    /// If set, this query is executed for each document against jq
    /// to produce a filename to store the resulting document.
    ///
    /// This option only produces a key for each filename. The key
    /// is evaluated against the base document and does not take the jq query
    /// into consideration.
    ///
    /// Example: --split '"./" + (.metadata.name) + "_" + (.kind | ascii_downcase) + ".yaml"'
    #[arg(
        short,
        long,
        conflicts_with = "in_place",
        // we need to be able to parse these back into documents to be able to split them
        conflicts_with = "raw_output",
        conflicts_with = "compact_output",
        conflicts_with = "join_output"
    )]
    split: Option<String>,

    /// Query to be sent to jq (see https://jqlang.github.io/jq/manual/)
    ///
    /// Default "."
    #[arg()]
    jq_query: Option<String>,

    /// Optional file to read (instead of stdin) in the chosen --input format
    #[arg()]
    file: Option<PathBuf>,

    // ----- jq arguments
    /// Compact instead of pretty-printed output (jq output only)
    ///
    /// This is unlikely to work with yaml or toml output because it requires
    /// that the jq -c output is deserializable into the desired output format.
    #[arg(short = 'c', long, default_value = "false")]
    compact_output: bool,

    /// Output strings without escapes and quotes (jq output only)
    ///
    /// This is unlikely to work with yaml or toml output because it requires
    /// that the jq -r output is deserializable into the desired output format.
    #[arg(short = 'r', long, default_value = "false")]
    raw_output: bool,

    /// Output strings without escapes and quotes, without newlines after each output (jq output only)
    ///
    /// This is unlikely to work with yaml or toml output because it requires
    /// that the jq -r output is deserializable into the desired output format.
    #[arg(short = 'j', long, default_value = "false")]
    join_output: bool,

    /// Search jq modules from the directory
    #[arg(short = 'L')]
    modules: Option<PathBuf>,
}

impl Args {
    fn jq_args(&self) -> Vec<String> {
        let mut args = vec![];
        if let Some(query) = &self.jq_query {
            args.push(query.into())
        }
        if self.compact_output {
            args.push("-c".into());
        }
        if self.raw_output {
            args.push("-r".into());
        }
        if self.join_output {
            args.push("-j".into());
        }
        if let Some(dir) = &self.modules {
            args.push("-L".into());
            args.push(format!("{}", dir.display()));
        }
        args
    }
    fn jq_split_args(&self) -> Option<Vec<String>> {
        let split_by = &self.split.as_ref()?;
        let mut args = vec!["-r".into()]; // we expect single unquoted keys
        args.push(split_by.to_string());
        Some(args)
    }

    fn read_yaml_docs(&mut self) -> Result<Vec<serde_json::Value>> {
        let yaml_de = if let Some(f) = &self.file {
            if !std::path::Path::new(&f).exists() {
                Self::try_parse_from(["cmd", "-h"])?;
                std::process::exit(2);
            }
            let file = std::fs::File::open(f)?;
            // NB: can do everything async (via tokio + tokio_util) except this:
            // serde only has a sync reader interface, so may as well do all sync.
            Deserializer::from_reader(BufReader::new(file))
        } else if !stdin().is_terminal() && !cfg!(test) {
            debug!("reading from stdin");
            Deserializer::from_reader(stdin())
        } else {
            Self::try_parse_from(["cmd", "-h"])?;
            std::process::exit(2);
        };

        let mut docs: Vec<serde_json::Value> = vec![];
        for doc in yaml_de {
            let json_value: serde_json::Value = {
                let mut yaml_doc: serde_yaml::Value = singleton_map_recursive::deserialize(doc)?;
                yaml_doc.apply_merge()?;
                let yaml_ser = serde_yaml::to_string(&yaml_doc)?;
                serde_yaml::from_str(&yaml_ser)?
            };
            docs.push(json_value);
        }
        debug!("found {} documents", docs.len());
        Ok(docs)
    }

    fn read_yaml(&mut self) -> Result<Vec<u8>> {
        // yaml is multidoc parsed by default, so flatten when <2 docs to conform to jq interface
        let docs = self.read_yaml_docs()?;
        // if there is 1 or 0 documents, do not return as nested documents
        let ser = match docs.as_slice() {
            [x] => serde_json::to_vec(x)?,
            [] => serde_json::to_vec(&serde_json::json!({}))?,
            xs => serde_json::to_vec(xs)?,
        };
        Ok(ser)
    }

    fn read_toml(&mut self) -> Result<serde_json::Value> {
        use toml::Table;
        let mut buf = String::new();
        let toml_str = if let Some(f) = &self.file {
            if !std::path::Path::new(&f).exists() {
                Self::try_parse_from(["cmd", "-h"])?;
                std::process::exit(2);
            }
            std::fs::read_to_string(f)?
        } else if !stdin().is_terminal() && !cfg!(test) {
            debug!("reading from stdin");
            stdin().read_to_string(&mut buf)?;
            buf
        } else {
            Self::try_parse_from(["cmd", "-h"])?;
            std::process::exit(2);
        };
        let doc: Table = toml_str.parse()?;
        let doc_as: serde_json::Value = doc.try_into()?;
        Ok(doc_as)
    }

    fn read_toml_docs(&mut self) -> Result<Vec<serde_json::Value>> {
        let toml = self.read_toml()?;
        // TODO: need toml crate to support multidoc +++ or something
        // see https://github.com/toml-lang/toml/issues/511
        // see https://github.com/toml-lang/toml/issues/583
        Ok(vec![toml]) // assume single document for now
    }

    fn read_json(&mut self) -> Result<serde_json::Value> {
        let json_value: serde_json::Value = if let Some(f) = &self.file {
            if !std::path::Path::new(&f).exists() {
                Self::try_parse_from(["cmd", "-h"])?;
                std::process::exit(2);
            }
            let file = std::fs::File::open(f)?;
            serde_json::from_reader(BufReader::new(file))?
        } else if !stdin().is_terminal() && !cfg!(test) {
            debug!("reading from stdin");
            serde_json::from_reader(stdin())?
        } else {
            Self::try_parse_from(["cmd", "-h"])?;
            std::process::exit(2);
        };
        Ok(json_value)
    }

    // multidoc equivalent reader interface for json
    fn read_json_docs(&mut self) -> Result<Vec<serde_json::Value>> {
        let json = self.read_json()?;
        // outermost is array? equivalent to multidoc
        if let serde_json::Value::Array(ary) = json {
            return Ok(ary);
        }
        // otherwise, it's 1 doc
        Ok(vec![json])
    }

    fn read_input(&mut self) -> Result<Vec<u8>> {
        let ser = match self.input {
            Input::Yaml => self.read_yaml()?,
            Input::Toml => serde_json::to_vec(&self.read_toml()?)?,
            Input::Json => serde_json::to_vec(&self.read_json()?)?,
        };
        debug!("input decoded as json: {}", String::from_utf8_lossy(&ser));
        Ok(ser)
    }
    fn read_input_multidoc(&mut self) -> Result<Vec<serde_json::Value>> {
        let ser = match self.input {
            Input::Yaml => self.read_yaml_docs()?,
            Input::Toml => self.read_toml_docs()?,
            Input::Json => self.read_json_docs()?,
        };
        //debug!("input decoded as json: {}", String::from_utf8_lossy(&ser));
        Ok(ser)
    }

    /// Pass json encoded bytes to jq with arguments for jq
    fn shellout(&self, input: &[u8], args: &[String]) -> Result<Vec<u8>> {
        debug!("jq args: {:?}", &args);
        // shellout jq with given args
        let mut child = Command::new("jq")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        // pass file input as stdin
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&input).unwrap();
        drop(stdin);
        // then wait for exit and gather output
        let output = child.wait_with_output()?;
        if !output.status.success() {
            anyhow::bail!("arguments rejected by jq: {}", output.status);
        }
        debug!("jq stdout: {}", String::from_utf8_lossy(&output.stdout));
        Ok(output.stdout)
    }

    // Convert stdout into one of the Output formats verbatim as a single string
    fn output(&self, stdout: Vec<u8>) -> Result<String> {
        match self.output {
            // Only jq output is guaranteed to succeed because it's not parsed as a format
            // if people pass -r to jq, then this can strip formats
            Output::Jq => {
                // NB: stdout here is not always json - users can pass -r to jq
                Ok(String::from_utf8_lossy(&stdout).trim_end().to_string())
            }
            // Other outputs are speculatively parsed as the requested formats
            Output::Yaml => {
                // handle multidoc from jq output (e.g. '.[].name' type queries on multidoc input)
                let docs = serde_json::Deserializer::from_slice(&stdout)
                    .into_iter::<serde_json::Value>()
                    .flatten()
                    .collect::<Vec<_>>();
                debug!("parsed {} documents", docs.len());
                let output = match docs.as_slice() {
                    [x] => serde_yaml::to_string(&x)?,
                    [] => serde_yaml::to_string(&serde_json::json!({}))?,
                    xs => serde_yaml::to_string(&xs)?,
                };
                Ok(output.trim_end().to_string())
            }
            Output::Toml => {
                let val: serde_json::Value = serde_json::from_slice(&stdout)?;
                Ok(toml::to_string(&val)?.trim_end().to_string())
            }
        }
    }
    // Convert stdout into one of the Output formats verbatim as multidoc strings
    // NB: not actually needed atm
    #[allow(unused)] // refactor later maybe
    fn output_matched(&self, stdout: Vec<u8>) -> Result<Vec<String>> {
        let docs = serde_json::Deserializer::from_slice(&stdout)
            .into_iter::<serde_json::Value>()
            .flatten()
            .collect::<Vec<_>>();
        debug!("parsed {} documents", docs.len());
        let mut res = vec![];
        // All formats are strictly parsed as the requested formats
        for x in docs.as_slice() {
            let str_doc: String = match self.output {
                // We even need jq output to be valid json in this case to allow multidoc to be matched up
                Output::Jq => serde_json::to_string_pretty(&x)?,
                Output::Yaml => serde_yaml::to_string(&x)?,
                Output::Toml => toml::to_string(&x)?,
            };
            res.push(str_doc.trim_end().to_string());
        }
        Ok(res)
    }
}

fn init_env_tracing_stderr() -> Result<()> {
    use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};
    let logger = tracing_subscriber::fmt::layer().compact().with_writer(stderr);
    let env_filter = EnvFilter::try_from_default_env().or(EnvFilter::try_new("info"))?;
    let collector = Registry::default().with(logger).with(env_filter);
    Ok(tracing::subscriber::set_global_default(collector)?)
}

fn main() -> Result<()> {
    init_env_tracing_stderr()?;
    let mut args = Args::parse();
    // Capture shortcuts manually due to https://github.com/clap-rs/clap/issues/3146
    if args.yaml_output {
        args.output = Output::Yaml;
    } else if args.toml_output {
        args.output = Output::Toml
    }
    debug!("args: {:?}", args);
    let jq_args = args.jq_args();
    if let Some(split_args) = &args.jq_split_args() {
        // File splitting mode. Requiring precise multidoc parsing and evaluation
        let inputs = args.read_input_multidoc()?;
        // Evaluate each document with the split expression against jq
        // Later on, we match up the array of filenames with the corresponding output
        for json_doc in inputs {
            let data = serde_json::to_vec(&json_doc)?;
            let splitout = args.shellout(&data, split_args)?;
            let key = String::from_utf8_lossy(&splitout).trim_end().to_string();
            // TODO: assert no linebreaks in keysplit - it should be used for a path construction

            let stdout = args.shellout(&data, &jq_args)?;
            let doc = args.output(stdout)?;
            // debug:
            let path = std::path::Path::new(".").join(key);
            debug!("Writing {}", path.display());
            let mut f = std::fs::File::create(path)?;
            f.write_all(doc.as_bytes())?;
        }
    } else {
        // normal, single pass mode on blob of u8 serde_json values passed to jq
        let input = args.read_input()?;
        let stdout = args.shellout(&input, &jq_args)?;
        // Lenient output mode (accept loose jq compact/join style output)
        let output = args.output(stdout)?;
        if args.in_place && args.file.is_some() {
            let f = args.file.unwrap(); // required
            std::fs::write(f, output + "\n")?;
        } else {
            // write result to stdout ignoring SIGPIPE errors
            // https://github.com/rust-lang/rust/issues/46016
            let _ = writeln!(std::io::stdout(), "{output}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn file_input_both_outputs() -> Result<()> {
        init_env_tracing_stderr()?;
        let mut args = Args {
            jq_query: Some(".[2].metadata".into()),
            compact_output: true,
            output: Output::Jq,
            file: Some("test/deploy.yaml".into()),
            ..Default::default()
        };
        println!("have stdin? {}", !std::io::stdin().is_terminal());
        let data = args.read_input().unwrap();
        println!("debug args: {:?}", args);
        let res = args.shellout(&data, &args.jq_args()).unwrap();
        let out = args.output(res)?;
        assert_eq!(out, "{\"name\":\"controller\"}");
        args.output = Output::Yaml;
        let res2 = args.shellout(&data, &args.jq_args())?;
        let out2 = args.output(res2)?;
        assert_eq!(out2, "name: controller");
        Ok(())
    }
}
