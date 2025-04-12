@echo off

cd c:\src\eva4\svc\controller-system
call c:\src\init.bat
%HOME%/.cargo/bin/cargo build --release -F agent-windows
