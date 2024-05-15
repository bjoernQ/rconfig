use serde::Deserialize;
pub use serde_json::Value;
use std::{collections::BTreeMap as Map, env, path::PathBuf};

#[derive(Deserialize, Debug)]
pub enum Error {
    InvalidKey,
    InvalidConfiguration(String),
}

#[derive(Deserialize, Debug, Clone)]
pub struct ConfigOption {
    pub description: String,
    #[serde(rename(deserialize = "type"))]
    pub value_type: Option<ValueType>,

    #[serde(default)]
    pub depends: Vec<Vec<String>>,
    pub values: Option<Vec<ValueItem>>,

    #[serde(rename(deserialize = "default"))]
    pub default_value: Option<Value>,

    pub options: Option<Map<String, ConfigOption>>,

    pub __value: Option<Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ValueItem {
    pub description: String,
    pub value: Value,
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
) -> Result<Vec<(String, String)>, Error> {
    let input = basic_toml::from_str::<Value>(input).unwrap();

    let input = input.as_object().unwrap().get(crate_name).unwrap();

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
        let take = is_valid_depends(&item.depends, all_config, features);

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
        let take = take && is_valid_depends(&item.depends, all_config, features);

        if let Some(_value) = &item.__value {
            if !take {
                return Err(Error::InvalidConfiguration(name.to_string()));
            }
        }

        if let Some(options) = item.options.as_ref() {
            validate(options, all_config, features, take)?;
        }
    }

    Ok(())
}

fn is_valid_depends(
    depends: &Vec<Vec<String>>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
) -> bool {
    for depend in depends {
        if !depend.iter().any(|d| features.contains(&d.as_str()))
            && !depend
                .iter()
                .any(|d| is_value_resolves_to_set(d, all_config))
        {
            return false;
        }
    }

    true
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
    result: &mut Vec<(String, String)>,
    config: &Map<String, ConfigOption>,
    all_config: &Map<String, ConfigOption>,
    features: &Vec<&str>,
    prefix: String,
) {
    for (name, item) in config {
        if let Some(value) = &item.__value {
            result.push((format!("{}{}", prefix, name), value.to_string()));
        } else {
            if let Some(value) = &item.default_value {
                if is_valid_depends(&item.depends, &all_config, features) {
                    result.push((format!("{}{}", prefix, name), value.to_string()));
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

    for (name, value) in cfg {
        let name = name.replace(".", "_");
        if value != "0" && value != "false" {
            println!("cargo::rustc-cfg={name}");
        }
        println!("cargo::rustc-env=CONFIG_{name}={value}");
    }
}

pub fn load_config(definition: &str, crate_name: &str) -> Vec<(String, String)> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let root_path = find_root_path(&out_dir);
    let cfg_path = root_path.clone();

    let cfg_path = cfg_path.as_ref().and_then(|c| {
        let mut x = c.to_owned();
        x.push("config.toml");
        Some(x)
    });

    let cfg_path = cfg_path.unwrap();
    let config = std::fs::read_to_string(&cfg_path).unwrap();

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
    depends = [["esp32", "esp32s2", "esp32s3"]]
    
    # something with a type is something which can be configured
    [psram.options.enable]
    description = "Enable PSRAM"
    type = "bool"
    default = false
    
    [psram.options.size]
    description = "PSRAM Size"
    depends = [["psram.enable"]]
    type = "enum"
    values = [
        { description = "1MB", value = 1 },
        { description = "2MB", value = 2 },
        { description = "4MB", value = 4 },
    ]
    default = 2
    
    [psram.options.type]
    description = "PSRAM Type"
    depends = [["esp32s3"],["psram.enable"]]
    
    [psram.options.type.options.type]
    description = "PSRAM Type"
    depends = [["esp32s3"]]
    type = "enum"
    values = [
        { description = "Quad", value = "quad" },
        { description = "Octal", value = "octal" },
    ]
    default = 2
    
    [heap]
    description = "Heapsize"
    
    [heap.options.size]
    description = "Bytes to allocate"
    type = "u32"
    min = 0
    max = 65536
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
            vec![("heap.size".to_string(), "30000".to_string())],
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
                ("heap.size".to_string(), "30000".to_string()),
                ("psram.enable".to_string(), "true".to_string()),
                ("psram.size".to_string(), "4".to_string()),
                ("psram.type.type".to_string(), "2".to_string()),
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
                ("heap.size".to_string(), "30000".to_string()),
                ("psram.enable".to_string(), "false".to_string()),
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
                ("heap.size".to_string(), "30000".to_string()),
                ("psram.enable".to_string(), "true".to_string()),
                ("psram.size".to_string(), "4".to_string()),
            ],
            effective_config
        );
    }

    #[test]
    fn current_config_result() {
        let cfg = r#"# something without a type is just a menu item
        [psram]
        description = "PSRAM"
        depends = [["esp32", "esp32s2", "esp32s3"]]
        
        # something with a type is something which can be configured
        [psram.options.enable]
        description = "Enable PSRAM"
        type = "bool"
        default = false
        __value = true
        
        [psram.options.size]
        description = "PSRAM Size"
        depends = [["psram.enable"]]
        type = "enum"
        values = [
            { description = "1MB", value = 1 },
            { description = "2MB", value = 2 },
            { description = "4MB", value = 4 },
        ]
        default = 2
        __value = 4
        
        [psram.options.type]
        description = "PSRAM Type"
        depends = [["esp32s3"],["psram.enable"]]
        
        [psram.options.type.options.type]
        description = "PSRAM Type"
        depends = [["esp32s3"]]
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
        min = 0
        max = 65536
        __value = 4949
        "#;

        let parsed_definition = parse_definition_str(cfg);
        let effective_config = current_config_values(parsed_definition, vec!["esp32s3"]).unwrap();

        println!("{:#?}", effective_config);

        assert_eq!(
            vec![
                ("heap.size".to_string(), "4949".to_string()),
                ("psram.enable".to_string(), "true".to_string()),
                ("psram.size".to_string(), "4".to_string()),
                ("psram.type.type".to_string(), "\"octal\"".to_string()),
            ],
            effective_config
        );
    }
}
