# Ghidra headless script to export decompiled C code
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor
import os

output_dir = "/Users/xaviergeerinck/Projects/insta360/decompiled"

program = getCurrentProgram()
prog_name = program.getName()

decomp = DecompInterface()
decomp.openProgram(program)

monitor = ConsoleTaskMonitor()
func_manager = program.getFunctionManager()
funcs = func_manager.getFunctions(True)

output_file = os.path.join(output_dir, prog_name + ".c")
with open(output_file, "w") as f:
    f.write("// Decompiled: %s\n\n" % prog_name)
    count = 0
    for func in funcs:
        if monitor.isCancelled():
            break
        results = decomp.decompileFunction(func, 30, monitor)
        if results and results.decompileCompleted():
            code = results.getDecompiledFunction()
            if code:
                body = code.getC()
                f.write("// Function at 0x%s\n" % func.getEntryPoint())
                f.write(body)
                f.write("\n\n")
                count += 1
    f.write("// Total functions decompiled: %d\n" % count)

print("Decompiled %d functions to %s" % (count, output_file))
