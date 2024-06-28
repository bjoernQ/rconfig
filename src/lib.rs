use convert_case::Casing;
use linked_hash_map::LinkedHashMap as Map;
use rhai::Engine;
use rhai::Scope;
use serde::Deserialize;
pub use serde_json::Map as JsonMap;
pub use serde_json::Value;
use std::io::Write;
use std::{env, path::PathBuf};

#[derive(Deserialize, Debug)]
pub enum Error {
    InvalidKey,
    InvalidConfiguration(String),
    InvalidConfigurationValue(String),
}

#[derive(Deserialize, Debug, Clone)]
pub struct ConfigOption {
    pub description: String,
    #[serde(rename(deserialize = "type"))]
    pub value_type: Option<ValueType>,

    pub depends: Option<String>,
    pub valid: Option<String>,

    pub values: Option<Vec<ValueItem>>,

    #[serde(rename(deserialize = "default"))]
    pub default_value: Option<Value>,

    pub options: Option<Map<String, ConfigOption>>,

    pub __value: Option<Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ValueItem {
    pub description: String,
    pub value: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub enum ValueType {
    #[serde(rename(deserialize = "bool"))]
    Bool,
    #[serde(rename(deserialize = "u32"))]
    U32,
    #[serde(rename(deserialize = "enum"))]
    Enum,
    #[serde(rename(deserialize = "string"))]
    String,
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueType::Bool => write!(f, "bool"),
            ValueType::U32 => write!(f, "u32"),
            ValueType::Enum => write!(f, "enum"),
            ValueType::String => write!(f, "string"),
        }
    }
}

#[cfg(not(host_os = "windows"))]
#[macro_export]
macro_rules! include_config {
    () => {
        include!(concat!(env!("OUT_DIR"), "/config.rs"));
    };
}

#[cfg(host_os = "windows")]
#[macro_export]
macro_rules! include_config {
    () => {
        include!(concat!(env!("OUT_DIR"), "\\config.rs"));
    };
}

pub fn parse_definition_str(input: &str) -> Map<String, ConfigOption> {
    basic_toml::from_str(input).unwrap()
}

pub fn evaluate_config_str_to_cfg(
    input: &str,
    crate_name: &str,
    mut config: Map<String, ConfigOption>,
    features: Vec<&str>,
) -> Result<Map<String, ConfigOption>, Error> {
    let input = basic_toml::from_str::<Value>(input).unwrap();

    let input = input.as_object().unwrap().get(crate_name).unwrap();

    // fuse the user changed configs into the config
    fuse(input.clone(), &mut config)?;

    // don't validate - might run into issue while editing and we'll remove things in the next step anyways

    let config = remove_non_applicable(&config, &config, &features, Map::new())?;

    Ok(config)
}

pub fn evaluate_config_str(
    input: &str,
    crate_name: &str,
    mut config: Map<String, ConfigOption>,
    features: Vec<&str>,
) -> Result<Vec<(String, String, ValueType)>, Error> {
    let input = basic_toml::from_str::<Value>(input).unwrap();
    let no_input = basic_toml::from_str::<Value>("").unwrap();

    let input = input
        .as_object()
        .unwrap()
        .get(crate_name)
        .unwrap_or_else(|| &no_input);

    // fuse the user changed configs into the config
    fuse(input.clone(), &mut config)?;

    validate(&config, &config, &features, true)?;

    let config = remove_non_applicable(&config, &config, &features, Map::new())?;

    // create result
    let mut result = Vec::new();
    create_result(&mut result, &config, &config, &features, "".to_string());

    Ok(result)
}

pub fn current_config_values(
    config: Map<String, ConfigOption>,
    features: Vec<&str>,
) -> Result<Vec<(String, String)>, Error> {
    let config = remove_non_applicable(&config, &config, &features, Map::new())?;

    // create result
    let mut result = Vec::new();
    create_current_config_result(&mut result, &config, &config, &features, "".to_string());

    Ok(result)
}

fn create_current_config_result(
    result: &mut Vec<(String, String)>,
    config: &Map<String, ConfigOption>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
    prefix: String,
) {
    for (name, item) in config {
        if let Some(value) = &item.__value {
            result.push((format!("{}{}", prefix, name), value.to_string()));
        } else if let Some(options) = item.options.as_ref() {
            create_current_config_result(
                result,
                options,
                all_config,
                features,
                format!("{}{}.", prefix, name),
            );
        }
    }
}

fn remove_non_applicable(
    config_part: &Map<String, ConfigOption>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
    mut building: Map<String, ConfigOption>,
) -> Result<Map<String, ConfigOption>, Error> {
    for (name, item) in config_part {
        let mut item = item.clone();
        let take = is_valid_depends(item.depends.clone(), all_config, features);

        if let Some(options) = item.options.as_ref() {
            let options = remove_non_applicable(options, all_config, features, Map::new())?;
            item.options = Some(options);
        }

        if take {
            building.insert(name.clone(), item);
        }
    }

    Ok(building)
}

fn validate(
    config_part: &Map<String, ConfigOption>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
    take: bool,
) -> Result<(), Error> {
    for (name, item) in config_part {
        let take = take && is_valid_depends(item.depends.clone(), all_config, features);

        if let Some(_value) = &item.__value {
            if !take {
                return Err(Error::InvalidConfiguration(name.to_string()));
            }

            if !is_value_valid(item.valid.clone(), _value, all_config, features) {
                return Err(Error::InvalidConfigurationValue(name.to_string()));
            }
        }

        if let Some(options) = item.options.as_ref() {
            validate(options, all_config, features, take)?;
        }
    }

    Ok(())
}

fn is_value_valid(
    validation: Option<String>,
    value: &Value,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
) -> bool {
    if let Some(validation) = validation {
        // is this expensive? should we reuse the Engine?
        let mut engine = Engine::new();

        let script_features: Vec<String> = features.iter().map(|s| s.to_string()).collect();

        let f = move |what: String| script_features.contains(&what);
        engine.register_fn("feature", f);

        let all_config = all_config.clone();
        let f = move |what: &str| is_value_resolves_to_set(what, &all_config);
        engine.register_fn("enabled", f);

        let mut scope = Scope::new();
        match value {
            Value::Bool(b) => scope.push("value", *b),
            Value::Number(n) => scope.push("value", n.as_u64().unwrap() as i64),
            Value::String(s) => scope.push("value", s.as_str().to_string()),
            _ => scope.push("value", false),
        };

        engine
            .eval_with_scope::<bool>(&mut scope, &validation)
            .unwrap()
    } else {
        true
    }
}

fn is_valid_depends(
    depends: Option<String>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
) -> bool {
    if let Some(depends) = depends {
        // is this expensive? should we reuse the Engine?
        let mut engine = Engine::new();

        let script_features: Vec<String> = features.iter().map(|s| s.to_string()).collect();

        let f = move |what: String| script_features.contains(&what);
        engine.register_fn("feature", f);

        let all_config = all_config.clone();
        let f = move |what: &str| is_value_resolves_to_set(what, &all_config);
        engine.register_fn("enabled", f);

        engine.eval::<bool>(&depends).unwrap()
    } else {
        true
    }
}

fn is_value_resolves_to_set(option: &str, all_config: &Map<String, ConfigOption>) -> bool {
    let value = get_value(option, all_config);
    match value {
        None => false,
        Some(value) => match value {
            Value::Null => false,
            Value::Bool(value) => value,
            Value::Number(value) => value != serde_json::Number::from_f64(0f64).unwrap(),
            Value::String(value) => !value.is_empty(),
            Value::Array(_) => false,
            Value::Object(_) => false,
        },
    }
}

fn get_value(option: &str, all_config: &Map<String, ConfigOption>) -> Option<serde_json::Value> {
    let path = option.split(".");
    let mut current = all_config;

    let parts: Vec<&str> = path.collect();
    for part in &parts[..parts.len() - 1] {
        if let Some(next) = &current.get(*part) {
            if let Some(next) = next.options.as_ref() {
                current = next;
            } else {
                return None;
            }
        } else {
            return None;
        }
    }

    if current.get(*parts.last().unwrap()).as_ref().is_some() {
        let value = current
            .get(*parts.last().unwrap())
            .as_ref()
            .unwrap()
            .__value
            .clone();
        let def_value = current
            .get(*parts.last().unwrap())
            .as_ref()
            .unwrap()
            .default_value
            .clone();

        let value = if value.is_some() { value } else { def_value };
        value
    } else {
        None
    }
}

fn create_result(
    result: &mut Vec<(String, String, ValueType)>,
    config: &Map<String, ConfigOption>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
    prefix: String,
) {
    for (name, item) in config {
        if let Some(value) = &item.__value {
            result.push((
                format!("{}{}", prefix, name),
                value.to_string(),
                item.value_type.as_ref().unwrap().clone(),
            ));
        } else {
            if let Some(value) = &item.default_value {
                if is_valid_depends(item.depends.clone(), &all_config, features) {
                    result.push((
                        format!("{}{}", prefix, name),
                        value.to_string(),
                        item.value_type.as_ref().unwrap().clone(),
                    ));
                }
            } else {
                if let Some(options) = item.options.as_ref() {
                    create_result(
                        result,
                        options,
                        all_config,
                        features,
                        format!("{}{}.", prefix, name),
                    );
                }
            }
        }
    }
}

fn fuse(value: Value, config: &mut Map<String, ConfigOption>) -> Result<(), Error> {
    match value {
        Value::Null => (),
        Value::Bool(_) => todo!(),
        Value::Number(_) => todo!(),
        Value::String(_) => todo!(),
        Value::Array(_) => todo!(),
        Value::Object(item) => {
            for (name, value) in item {
                if !config.contains_key(&name) {
                    return Err(Error::InvalidKey);
                }
                let c = config.get_mut(&name).unwrap();

                if let Some(options) = c.options.as_mut() {
                    fuse(value, options)?;
                } else {
                    c.__value = Some(value);
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct EnumDefinition {
    name: String,
    variant_names: Vec<String>,
}

fn extract_all_enum_definitions(config: Map<String, ConfigOption>) -> Vec<EnumDefinition> {
    let mut result = Vec::new();

    extract_all_enum_definitions_recusive(&mut result, &config, "".to_string());

    result
}

fn extract_all_enum_definitions_recusive(
    result: &mut Vec<EnumDefinition>,
    config: &Map<String, ConfigOption>,
    prefix: String,
) {
    for (name, item) in config {
        if let Some(ValueType::Enum) = item.value_type {
            let mut variant_names = Vec::new();
            for variant in item.values.as_ref().unwrap() {
                variant_names.push(to_variant_name(&variant.value));
            }

            let item = EnumDefinition {
                name: format!(
                    "{}{}",
                    prefix.to_case(convert_case::Case::Pascal),
                    name.to_case(convert_case::Case::Pascal)
                ),
                variant_names,
            };
            result.push(item);
        } else {
            if let Some(options) = item.options.as_ref() {
                extract_all_enum_definitions_recusive(
                    result,
                    options,
                    format!(
                        "{}{}",
                        prefix.to_case(convert_case::Case::Pascal),
                        name.to_case(convert_case::Case::Pascal)
                    ),
                );
            }
        }
    }
}

pub fn to_variant_name(str: &str) -> String {
    let str = if str.chars().next().unwrap().is_numeric() {
        format!("Variant{}", str)
    } else {
        str.to_string()
    };

    str.to_case(convert_case::Case::Pascal)
}

pub fn apply_config(definition: &PathBuf) {
    // for tooling
    println!(
        "cargo::rustc-env=__RCONFIG={}",
        definition
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
            .trim_start_matches("\\\\?\\")
    );

    let crate_name = env::var("CARGO_PKG_NAME").unwrap();
    println!("cargo::rustc-env=__RCONFIG_CRATE={}", crate_name);

    let definition = std::fs::read_to_string(definition).unwrap();

    let cfg = load_config(&definition, &crate_name);

    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut config_rs = std::fs::File::create(out.join("config.rs")).unwrap();

    let enums = extract_all_enum_definitions(parse_definition_str(&definition));
    for e in enums {
        config_rs
            .write("#[derive(Debug,Clone,Copy)]\n".as_bytes())
            .unwrap();
        config_rs
            .write(format!("pub enum {} {{\n", &e.name).as_bytes())
            .unwrap();
        for v in e.variant_names {
            config_rs.write(format!("{},\n", &v).as_bytes()).unwrap();
        }
        config_rs.write("}\n".as_bytes()).unwrap();
    }

    for (name, value, value_type) in cfg {
        eprintln!("{name}");
        let name = name.replace(".", "_");
        println!("cargo::rustc-cfg=has_{name}");
        if value != "0" && value != "false" {
            println!("cargo::rustc-cfg={name}");
        }

        if value_type != ValueType::Enum {
            config_rs
                .write(
                    format!(
                        "pub const {}: {} = {};\n",
                        name.to_uppercase(),
                        value_type.to_string(),
                        value
                    )
                    .as_bytes(),
                )
                .unwrap();
        } else {
            config_rs
                .write(
                    format!(
                        "pub const {}: {} = {}::{};\n",
                        name.to_uppercase(),
                        to_variant_name(&name),
                        to_variant_name(&name),
                        to_variant_name(&value.replace("\"", "")),
                    )
                    .as_bytes(),
                )
                .unwrap();
        }
    }
}

pub fn load_config(definition: &str, crate_name: &str) -> Vec<(String, String, ValueType)> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let root_path = find_root_path(&out_dir);
    let cfg_path = root_path.clone();

    let cfg_path = cfg_path.as_ref().and_then(|c| {
        let mut x = c.to_owned();
        x.push("config.toml");
        Some(x)
    });

    let cfg_path = cfg_path.unwrap();
    let config = if let Ok(metadata) = std::fs::metadata(&cfg_path) {
        if metadata.is_file() {
            std::fs::read_to_string(&cfg_path).unwrap()
        } else {
            "".to_string()
        }
    } else {
        "".to_string()
    };

    println!("cargo::rerun-if-changed={}", cfg_path.to_str().unwrap());

    let parsed_definition = parse_definition_str(definition);

    // collect features
    let vars = env::vars();
    let mut features = Vec::new();
    for (var, _) in vars {
        if var.starts_with("CARGO_FEATURE_") {
            let var = var
                .strip_prefix("CARGO_FEATURE_")
                .unwrap()
                .to_ascii_lowercase()
                .replace("_", "-");
            features.push(var);
        }
    }

    // for tooling
    println!("cargo::rustc-env=__RCONFIG_FEATURES={}", features.join(","));

    let effective_config = evaluate_config_str(
        &config,
        crate_name,
        parsed_definition,
        features.iter().map(|v| v.as_str()).collect(),
    )
    .unwrap();

    effective_config
}

fn find_root_path(out_dir: &PathBuf) -> Option<PathBuf> {
    // clean out_dir by removing all trailing directories, until it ends with target
    let mut out_dir = PathBuf::from(out_dir);

    // TODO better also check `CARGO_TARGET_DIR` to know if the user wants a relocated target dir
    // OR use `CARGO_MANIFEST_DIR`?
    while !out_dir.ends_with("target") {
        if !out_dir.pop() {
            // We ran out of directories...
            return None;
        }
    }

    out_dir.pop();

    Some(out_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITION: &str = r#"# something without a type is just a menu item
    [psram]
    description = "PSRAM"
    depends = "feature(\"esp32\") || feature(\"esp32s2\") || feature(\"esp32s3\")"
    
    # something with a type is something which can be configured
    [psram.options.enable]
    description = "Enable PSRAM"
    type = "bool"
    default = false
    
    [psram.options.size]
    description = "PSRAM Size"
    depends = "enabled(\"psram.enable\")"
    type = "enum"
    values = [
        { description = "1MB", value = "1" },
        { description = "2MB", value = "2" },
        { description = "4MB", value = "4" },
    ]
    default = "2"
    
    [psram.options.type]
    description = "PSRAM Type"
    depends = "feature(\"esp32s3\") && enabled(\"psram.enable\")"
    
    [psram.options.type.options.type]
    description = "PSRAM Type"
    depends = "feature(\"esp32s3\")"
    type = "enum"
    values = [
        { description = "Quad", value = "quad" },
        { description = "Octal", value = "octal" },
    ]
    default = "quad"
    
    [heap]
    description = "Heapsize"
    
    [heap.options.size]
    description = "Bytes to allocate"
    type = "u32"
    valid = "value >= 0 && value <= 80000"
    "#;

    #[test]
    fn parse_config1() {
        let cfg = r#"
        [mycrate]
        #psram.enable = true
        #psram.size = 4
        #psram.type.type = 2

        heap.size = 30000
        "#;

        let parsed_definition = parse_definition_str(DEFINITION);
        let effective_config = evaluate_config_str(
            &cfg,
            "mycrate",
            parsed_definition,
            vec!["esp32c6", "flip-link"],
        )
        .unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![("heap.size".to_string(), "30000".to_string(), ValueType::U32)],
            effective_config
        );
    }

    #[test]
    fn parse_config2() {
        let cfg = r#"
        [mycrate]
        psram.enable = true
        psram.size = 4
        psram.type.type = 2

        heap.size = 30000
        "#;

        let parsed_definition = parse_definition_str(DEFINITION);
        let effective_config =
            evaluate_config_str(&cfg, "mycrate", parsed_definition, vec!["esp32s3"]).unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![
                (
                    "psram.enable".to_string(),
                    "true".to_string(),
                    ValueType::Bool
                ),
                ("psram.size".to_string(), "4".to_string(), ValueType::Enum),
                (
                    "psram.type.type".to_string(),
                    "2".to_string(),
                    ValueType::Enum
                ),
                ("heap.size".to_string(), "30000".to_string(), ValueType::U32),
            ],
            effective_config
        );
    }

    #[test]
    fn parse_config2_2() {
        let cfg = r#"
        [mycrate]
        heap.size = 30000
        "#;

        let parsed_definition = parse_definition_str(DEFINITION);
        let effective_config =
            evaluate_config_str(&cfg, "mycrate", parsed_definition, vec!["esp32s3"]).unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![
                (
                    "psram.enable".to_string(),
                    "false".to_string(),
                    ValueType::Bool
                ),
                ("heap.size".to_string(), "30000".to_string(), ValueType::U32),
            ],
            effective_config
        );
    }

    #[test]
    fn parse_config3() {
        let cfg = r#"
        [mycrate]
        psram.enable = true
        psram.size = 4

        heap.size = 30000
        "#;

        let parsed_definition = parse_definition_str(DEFINITION);
        let effective_config =
            evaluate_config_str(&cfg, "mycrate", parsed_definition, vec!["esp32"]).unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![
                (
                    "psram.enable".to_string(),
                    "true".to_string(),
                    ValueType::Bool
                ),
                ("psram.size".to_string(), "4".to_string(), ValueType::Enum),
                ("heap.size".to_string(), "30000".to_string(), ValueType::U32),
            ],
            effective_config
        );
    }

    #[test]
    fn current_config_result() {
        let cfg = r#"# something without a type is just a menu item
        [psram]
        description = "PSRAM"
        depends = "feature(\"esp32\") || feature(\"esp32s2\") || feature(\"esp32s3\")"
        
        # something with a type is something which can be configured
        [psram.options.enable]
        description = "Enable PSRAM"
        type = "bool"
        default = false
        __value = true
        
        [psram.options.size]
        description = "PSRAM Size"
        depends = "enabled(\"psram.enable\")"
        type = "enum"
        values = [
            { description = "1MB", value = "1" },
            { description = "2MB", value = "2" },
            { description = "4MB", value = "4" },
        ]
        default = "2"
        __value = "4"
        
        [psram.options.type]
        description = "PSRAM Type"
        depends = "feature(\"esp32s3\") && enabled(\"psram.enable\")"
        
        [psram.options.type.options.type]
        description = "PSRAM Type"
        depends = "feature(\"esp32s3\")"
        type = "enum"
        values = [
            { description = "Quad", value = "quad" },
            { description = "Octal", value = "octal" },
        ]
        default = "quad"
        __value =  "octal"
        
        [heap]
        description = "Heapsize"
        
        [heap.options.size]
        description = "Bytes to allocate"
        type = "u32"
        valid = "value >= 0 && value <= 80000"
        __value = 4949
        "#;

        let parsed_definition = parse_definition_str(cfg);
        let effective_config = current_config_values(parsed_definition, vec!["esp32s3"]).unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![
                ("psram.enable".to_string(), "true".to_string()),
                ("psram.size".to_string(), "\"4\"".to_string()),
                ("psram.type.type".to_string(), "\"octal\"".to_string()),
                ("heap.size".to_string(), "4949".to_string()),
            ],
            effective_config
        );
    }
}
