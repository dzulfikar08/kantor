fn main() {
    let src = std::env::args().nth(1).unwrap_or_else(|| "1 + 1".to_owned());
    #[cfg(feature = "js")]
    {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        let v: i32 = ctx.with(|ctx| ctx.eval(src.as_bytes()).unwrap());
        println!("js {v}");
    }
    #[cfg(feature = "py")]
    {
        let interp = rustpython_vm::Interpreter::without_stdlib(Default::default());
        interp.enter(|vm| {
            let scope = vm.new_scope_with_builtins();
            let r = vm.run_block_expr(scope, &src).unwrap();
            println!("py {:?}", r.try_into_value::<i64>(vm).ok());
        });
    }
    #[cfg(not(any(feature = "js", feature = "py")))]
    println!("plain {src}");
}
