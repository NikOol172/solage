// solage_core/src/script.rs

use rhai::packages::Package;
use rhai::{Engine, Scope};
use std::collections::HashMap;
use std::process::Command;

pub struct ScriptContext {
    pub(crate) scope: Scope<'static>,
}

impl ScriptContext {
    pub fn new() -> Self {
        Self { scope: Scope::new() }
    }
}

pub struct ScriptEngine {
    engine: Engine,
}

impl ScriptEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();
        let package = rhai::packages::BasicMathPackage::new();
        package.register_into_engine(&mut engine);
        
        engine.on_print(|msg| { println!("[SCRIPT] {}", msg); });
        engine.on_debug(|msg, src, pos| { println!("[DEBUG] {} ({:?} {:?})", msg, src, pos); });
        engine.register_fn("exec", |cmd: &str| {
            #[cfg(target_os = "windows")] let _ = Command::new("cmd").args(["/C", cmd]).spawn();
            #[cfg(not(target_os = "windows"))] let _ = Command::new("sh").arg("-c").arg(cmd).spawn();
        });
        Self { engine }
    }

    pub fn build_context(&self, context: &HashMap<String, String>) -> ScriptContext {
        let mut scope = Scope::new();
        self.inject_context(&mut scope, context);
        ScriptContext { scope }
    }

    pub fn eval_with_context(&self, expr: &str, ctx: &mut ScriptContext, local_value: Option<&str>) -> Option<String> {
        let rewind_count = if let Some(val) = local_value {
            if let Ok(num) = val.parse::<f64>() {
                ctx.scope.push("value", num);
            } else if let Ok(b) = val.parse::<bool>() {
                ctx.scope.push("value", b);
            } else {
                ctx.scope.push("value", val.to_string());
            }
            1
        } else {
            0
        };

        let result = if let Ok(res) = self.engine.eval_with_scope::<i64>(&mut ctx.scope, expr) {
            Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<f64>(&mut ctx.scope, expr) {
            Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<bool>(&mut ctx.scope, expr) {
             Some(res.to_string())
        } else if let Ok(res) = self.engine.eval_with_scope::<String>(&mut ctx.scope, expr) {
            Some(res)
        } else {
            None
        };

        ctx.scope.rewind(ctx.scope.len() - rewind_count);
        
        result
    }

    pub fn run_action(&self, script: &str, context: &HashMap<String, String>) {
        let mut scope = Scope::new();
        self.inject_context(&mut scope, context);
        let _ = self.engine.run_with_scope(&mut scope, script);
    }

    pub fn validate(&self, value: &str, rule: &str) -> bool {
        let mut scope = Scope::new();
        if let Ok(num) = value.parse::<f64>() { scope.push("value", num); } 
        else { scope.push("value", value.to_string()); }
        self.engine.eval_with_scope::<bool>(&mut scope, rule).unwrap_or(false)
    }

    fn inject_context(&self, scope: &mut Scope, context: &HashMap<String, String>) {
        for (k, v) in context {
            if let Ok(i) = v.parse::<i64>() { scope.push(k, i); }
            else if let Ok(f) = v.parse::<f64>() { scope.push(k, f); }
            else if let Ok(b) = v.parse::<bool>() { scope.push(k, b); }
            else { scope.push(k, v.clone()); }
        }
    }
}
