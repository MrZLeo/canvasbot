use crate::builtin::{create_builtin_registry, BuiltinRegistry};
use indexmap::IndexMap;
use log::{error, info};
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::sync::{Arc, Mutex};
use toml::Value;

#[derive(Debug, Deserialize)]
pub struct Pipeline {
    pub variables: HashMap<String, Option<Value>>,
    pub steps: IndexMap<String, Step>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Command {
    Builtin {
        action: String,
        args: Option<Vec<String>>,
        abort_on_failure: Option<bool>,
    },
    Custom {
        action: String,
        args: Option<Vec<String>>,
        abort_on_failure: Option<bool>,
    },
    Variable {
        operation: String,
        name: String,
        value: i32,
    },
}

#[derive(Debug, Deserialize)]
pub struct Step {
    pub commands: Vec<Command>,
}

pub fn parse_config(file_path: &str) -> Pipeline {
    let config_content = fs::read_to_string(file_path).expect("Failed to read config file");
    toml::from_str(&config_content).expect("Failed to parse TOML")
}

pub struct Task {
    name: String,
    commands: Vec<Command>,
    variables: Arc<Mutex<HashMap<String, Option<Value>>>>,
}

pub struct Worker {
    tasks: Vec<Task>,
    pub results: IndexMap<String, String>,
    pub variables: Arc<Mutex<HashMap<String, Option<Value>>>>,
}

impl Worker {
    pub fn new(vars: HashMap<String, Option<Value>>) -> Worker {
        Worker {
            tasks: vec![],
            results: IndexMap::new(),
            variables: Arc::new(Mutex::new(vars)),
        }
    }

    pub fn modify_variable(&mut self, name: &str, value: Value) {
        self.variables
            .lock()
            .expect("Failed to lock variables")
            .entry(name.to_string())
            .and_modify(|e| {
                *e = Some(value);
            });
    }

    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(task);
    }

    pub async fn run(&mut self) {
        let builtin = create_builtin_registry();
        for task in &mut self.tasks {
            match task.run(&builtin).await {
                Ok(msg) => {
                    info!("Task executed successfully: {}", msg);
                    self.results.insert(task.name.clone(), msg);
                }
                Err(err) => {
                    error!("Error running task: {}", err);
                    self.results.insert(task.name.clone(), err.to_string());
                    break;
                }
            }
        }
    }
}

impl Task {
    pub fn new(
        name: String,
        commands: Vec<Command>,
        variables: Arc<Mutex<HashMap<String, Option<Value>>>>,
    ) -> Task {
        Task {
            name,
            commands,
            variables,
        }
    }

    pub async fn run(&mut self, builtin: &BuiltinRegistry) -> Result<String, Box<dyn Error>> {
        info!("Running task: {}", self.name);
        // 定义每列的宽度
        let label_width = 10;
        let status_width = 20;
        let label = format!("[{}]", self.name);

        for command in &self.commands {
            match command {
                Command::Builtin {
                    action,
                    args,
                    abort_on_failure,
                } => {
                    let mut args = args.clone().unwrap_or_default();
                    args.iter_mut()
                        .filter(|arg| arg.starts_with("var::"))
                        .for_each(|arg| {
                            *arg = self
                                .variables
                                .lock()
                                .expect("should be able to lock the varibales")
                                .get(&arg.replace("var::", ""))
                                .expect("should have varibales")
                                .as_ref()
                                .unwrap()
                                .to_string()
                                .trim_matches('\"')
                                .to_string();
                        });
                    info!("Running builtin command: {} with ({:?})", action, args);

                    if let Err(e) = builtin.execute(action, args).await {
                        error!("Error executing builtin command '{}': {}", action, e);
                        if abort_on_failure.unwrap_or(false) {
                            error!("Aborting task due to failure");
                            return Err(format!(
                                "{:<width$} {:>width2$}\n{}\nTest aborted.\n",
                                label,
                                "Failed",
                                e,
                                width = label_width,
                                width2 = status_width
                            )
                            .into());
                        } else {
                            return Ok(format!(
                                "{:<width$} {:>width2$}\n{}",
                                label,
                                "Failed",
                                e,
                                width = label_width,
                                width2 = status_width
                            ));
                        }
                    }
                }
                Command::Custom {
                    action,
                    args,
                    abort_on_failure,
                } => {
                    let mut args = args.clone().unwrap_or_default();
                    args.iter_mut()
                        .filter(|arg| arg.starts_with("var::"))
                        .for_each(|arg| {
                            *arg = self
                                .variables
                                .lock()
                                .expect("should be able to lock the varibales")
                                .get(&arg.replace("var::", ""))
                                .expect("should have varibales")
                                .as_ref()
                                .unwrap()
                                .to_string()
                                .trim_matches('\"')
                                .to_string();
                        });
                    info!("Running custom command: {} with ({:?})", action, args);

                    let root_dir = env!("CARGO_MANIFEST_DIR");
                    let cmd = std::process::Command::new(action.clone())
                        .args(&args[..])
                        .env("SEP_ROOT_DIR", root_dir)
                        .output();

                    match cmd {
                        Ok(output) => {
                            let stdout = std::str::from_utf8(&output.stdout).unwrap();
                            //println!("{}", stdout);

                            if !output.status.success() {
                                if abort_on_failure.unwrap_or(false) {
                                    error!("Aborting task due to failure");
                                    return Err(format!(
                                        "{:<width$} {:>width2$}\n{}\nTest aborted.\n",
                                        label,
                                        "Failed",
                                        stdout,
                                        width = label_width,
                                        width2 = status_width
                                    )
                                    .into());
                                } else {
                                    return Ok(format!(
                                        "{:<width$} {:>width2$}\n{}",
                                        label,
                                        "Failed",
                                        stdout,
                                        width = label_width,
                                        width2 = status_width
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error executing custom command '{}': {}", action, e);
                            if abort_on_failure.unwrap_or(false) {
                                error!("Aborting task due to failure");
                                return Err(
                                    format!("{} aborted due to failure: {}\n", label, e).into()
                                );
                            } else {
                                return Ok(format!("{} execution failed due to: {}\n", label, e));
                            }
                        }
                    }
                }
                Command::Variable {
                    operation,
                    name,
                    value,
                } => {
                    info!("Running variable command: {} {} {}", name, operation, value);
                    if operation == "+" {
                        let mut variables =
                            self.variables.lock().expect("Failed to lock variables");
                        if let Some(current_value) = variables.get_mut(name) {
                            match current_value {
                                Some(Value::Integer(num)) => {
                                    let current_int = *num;
                                    *current_value =
                                        Some(Value::Integer(current_int + *value as i64));
                                }
                                _ => {
                                    error!(
                                        "Variable {} is not an integer or is uninitialized",
                                        name
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(format!(
            "{:<width$} {:>width2$}\n",
            label,
            "Passed",
            width = label_width,
            width2 = status_width
        ))
    }
}
