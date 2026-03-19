// Ghidra decompile script
//@category Analysis

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompiledFunction;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;

import java.io.FileWriter;
import java.io.PrintWriter;

public class ghidra_decompile extends GhidraScript {
    @Override
    public void run() throws Exception {
        String outputDir = "/Users/xaviergeerinck/Projects/insta360/decompiled";
        String progName = currentProgram.getName();
        String outputFile = outputDir + "/" + progName + ".c";
        
        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);
        
        PrintWriter writer = new PrintWriter(new FileWriter(outputFile));
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
        writer.close();
        println("Decompiled " + count + " functions to " + outputFile);
    }
}
