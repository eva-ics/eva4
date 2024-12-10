@echo off

cd c:\src\eva4\svc\controller-system
call c:\src\init.bat
cargo build --release -F agent-windows
