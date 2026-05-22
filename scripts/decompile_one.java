// Ghidra headless post-script: decompile current program, write one .c file.
// First script arg = output directory.
//@category Analysis

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompiledFunction;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;

public class decompile_one extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length == 0) {
            throw new RuntimeException("missing output dir arg");
        }
        File outDir = new File(args[0]);
        outDir.mkdirs();
        String progName = currentProgram.getName();
        File outFile = new File(outDir, progName + ".c");

        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        PrintWriter writer = new PrintWriter(new FileWriter(outFile));
        try {
            writer.println("// Decompiled: " + progName);
            writer.println();
            FunctionIterator funcs = currentProgram.getFunctionManager().getFunctions(true);
            int count = 0;
            while (funcs.hasNext() && !monitor.isCancelled()) {
                Function func = funcs.next();
                DecompileResults results = decomp.decompileFunction(func, 30, monitor);
                if (results != null && results.decompileCompleted()) {
                    DecompiledFunction code = results.getDecompiledFunction();
                    if (code != null) {
                        writer.println("// Function at 0x" + func.getEntryPoint());
                        writer.println(code.getC());
                        writer.println();
                        count++;
                    }
                }
            }
            writer.println("// Total functions decompiled: " + count);
            println("[decompile_one] wrote " + outFile.getAbsolutePath() + " (" + count + " funcs)");
        } finally {
            writer.close();
        }
    }
}
