# Ghidra headless post-script: decompile the currently-loaded program
# and write a single <progname>.c into the directory given as the first script arg.
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor
import os, sys

args = getScriptArgs()
if not args:
    raise RuntimeError("missing output dir arg")
out_dir = args[0]
if not os.path.isdir(out_dir):
    os.makedirs(out_dir)

program = getCurrentProgram()
prog_name = program.getName()
out_path = os.path.join(out_dir, prog_name + ".c")

decomp = DecompInterface()
decomp.openProgram(program)
monitor = ConsoleTaskMonitor()

count = 0
with open(out_path, "w") as f:
    f.write("// Decompiled: %s\n\n" % prog_name)
    for func in program.getFunctionManager().getFunctions(True):
        if monitor.isCancelled():
            break
        results = decomp.decompileFunction(func, 30, monitor)
        if results and results.decompileCompleted():
            code = results.getDecompiledFunction()
            if code:
                f.write("// Function at 0x%s\n" % func.getEntryPoint())
                f.write(code.getC())
                f.write("\n\n")
                count += 1
    f.write("// Total functions decompiled: %d\n" % count)

print("[decompile_one] wrote %s (%d funcs)" % (out_path, count))
