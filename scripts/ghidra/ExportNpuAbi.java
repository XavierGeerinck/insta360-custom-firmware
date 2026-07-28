// Headless Ghidra post-script for focused, deterministic Wave NPU ABI evidence.
// Arguments:
//   0: output JSON path
//   1: comma-separated exact target function/symbol names
//   2: optional comma-separated diagnostic string fragments

import java.io.FileWriter;
import java.io.Writer;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.framework.Application;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.listing.Listing;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import ghidra.program.model.symbol.ReferenceManager;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;
import ghidra.program.model.symbol.SymbolTable;

public class ExportNpuAbi extends GhidraScript {
    private static final int DECOMPILE_TIMEOUT_SECONDS = 60;
    private static final int MAX_DECOMPILED_CHARS = 24000;
    private static final int MAX_DIAGNOSTIC_CHARS = 1000;
    private static final int MAX_DIRECT_CALLERS_PER_TARGET = 64;

    private static class Edge {
        String callsite;
        String function;
        String address;
        String kind;

        Edge(String callsite, String function, String address, String kind) {
            this.callsite = callsite;
            this.function = function;
            this.address = address;
            this.kind = kind;
        }
    }

    private static class Diagnostic {
        String from;
        String to;
        String value;

        Diagnostic(String from, String to, String value) {
            this.from = from;
            this.to = to;
            this.value = value;
        }
    }

    private static class FunctionEvidence {
        String role;
        String name;
        String qualified_name;
        String entry;
        long body_bytes;
        boolean external;
        boolean thunk;
        List<Edge> callers = new ArrayList<>();
        List<Edge> callees = new ArrayList<>();
        List<Diagnostic> diagnostics = new ArrayList<>();
        boolean decompile_success;
        String decompile_error;
        boolean decompile_truncated;
        String decompiled_c;
    }

    private static class TargetEvidence {
        String target;
        boolean found;
        List<String> symbol_addresses = new ArrayList<>();
        List<FunctionEvidence> functions = new ArrayList<>();

        TargetEvidence(String target) {
            this.target = target;
        }
    }

    private static class ProgramEvidence {
        int schema_version = 1;
        Map<String, Object> tool = new LinkedHashMap<>();
        Map<String, Object> program = new LinkedHashMap<>();
        List<TargetEvidence> targets = new ArrayList<>();
        List<String> warnings = new ArrayList<>();
    }

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            throw new IllegalArgumentException(
                "usage: ExportNpuAbi.java OUTPUT TARGETS [STRING_FILTERS]"
            );
        }

        String outputPath = args[0];
        List<String> targets = splitAndSort(args[1]);
        List<String> diagnosticFilters =
            args.length >= 3 ? splitAndSort(args[2]) : Collections.emptyList();
        if (targets.isEmpty()) {
            throw new IllegalArgumentException("target list must not be empty");
        }

        ProgramEvidence evidence = new ProgramEvidence();
        evidence.tool.put("name", "Ghidra");
        evidence.tool.put("version", Application.getApplicationVersion());
        evidence.tool.put("script", "ExportNpuAbi.java");
        evidence.program.put("name", currentProgram.getName());
        evidence.program.put("sha256", currentProgram.getExecutableSHA256());
        evidence.program.put(
            "language_id", currentProgram.getLanguageID().toString()
        );
        evidence.program.put(
            "compiler_spec_id",
            currentProgram.getCompilerSpec().getCompilerSpecID().toString()
        );
        evidence.program.put("format", currentProgram.getExecutableFormat());

        FunctionManager functionManager = currentProgram.getFunctionManager();
        SymbolTable symbolTable = currentProgram.getSymbolTable();
        DecompInterface decompiler = new DecompInterface();
        decompiler.toggleCCode(true);
        decompiler.toggleSyntaxTree(true);
        if (!decompiler.openProgram(currentProgram)) {
            evidence.warnings.add("decompiler failed to open program");
        }

        for (String target : targets) {
            TargetEvidence targetEvidence = new TargetEvidence(target);
            Set<Address> candidateAddresses = new LinkedHashSet<>();

            SymbolIterator symbols = symbolTable.getAllSymbols(true);
            while (symbols.hasNext() && !monitor.isCancelled()) {
                Symbol symbol = symbols.next();
                if (
                    target.equals(symbol.getName()) ||
                    target.equals(symbol.getName(true))
                ) {
                    candidateAddresses.add(symbol.getAddress());
                    targetEvidence.symbol_addresses.add(
                        symbol.getAddress().toString()
                    );
                }
            }

            FunctionIterator functions = functionManager.getFunctions(true);
            while (functions.hasNext() && !monitor.isCancelled()) {
                Function function = functions.next();
                if (
                    target.equals(function.getName()) ||
                    target.equals(function.getName(true))
                ) {
                    candidateAddresses.add(function.getEntryPoint());
                }
            }

            List<Address> sortedAddresses = new ArrayList<>(candidateAddresses);
            Collections.sort(sortedAddresses);
            Set<String> emittedEntries = new LinkedHashSet<>();
            List<Function> matchedFunctions = new ArrayList<>();
            for (Address address : sortedAddresses) {
                Function function = functionManager.getFunctionAt(address);
                if (function == null) {
                    function = functionManager.getFunctionContaining(address);
                }
                if (function == null) {
                    continue;
                }
                String entry = function.getEntryPoint().toString();
                if (!emittedEntries.add(entry)) {
                    continue;
                }
                matchedFunctions.add(function);
                targetEvidence.functions.add(
                    exportFunction(
                        function,
                        "target",
                        functionManager,
                        currentProgram.getListing(),
                        currentProgram.getReferenceManager(),
                        decompiler,
                        diagnosticFilters
                    )
                );
            }

            Set<Function> directCallers = new LinkedHashSet<>();
            for (Function function : matchedFunctions) {
                ReferenceIterator references =
                    currentProgram.getReferenceManager().getReferencesTo(
                        function.getEntryPoint()
                    );
                while (references.hasNext() && !monitor.isCancelled()) {
                    Reference reference = references.next();
                    if (!reference.getReferenceType().isCall()) {
                        continue;
                    }
                    Function caller = functionManager.getFunctionContaining(
                        reference.getFromAddress()
                    );
                    if (caller != null && !caller.isExternal()) {
                        directCallers.add(caller);
                    }
                }
            }
            List<Function> sortedCallers = new ArrayList<>(directCallers);
            sortedCallers.sort(
                Comparator.comparing(
                    value -> value.getEntryPoint().toString()
                )
            );
            int callerCount = 0;
            for (Function caller : sortedCallers) {
                if (callerCount >= MAX_DIRECT_CALLERS_PER_TARGET) {
                    evidence.warnings.add(
                        currentProgram.getName() + ":" + target +
                        " direct callers truncated at " +
                        MAX_DIRECT_CALLERS_PER_TARGET
                    );
                    break;
                }
                String entry = caller.getEntryPoint().toString();
                if (!emittedEntries.add(entry)) {
                    continue;
                }
                targetEvidence.functions.add(
                    exportFunction(
                        caller,
                        "direct-caller",
                        functionManager,
                        currentProgram.getListing(),
                        currentProgram.getReferenceManager(),
                        decompiler,
                        diagnosticFilters
                    )
                );
                callerCount++;
            }

            Collections.sort(targetEvidence.symbol_addresses);
            targetEvidence.functions.sort(
                Comparator
                    .comparing((FunctionEvidence value) -> value.entry)
                    .thenComparing(value -> value.qualified_name)
            );
            targetEvidence.found =
                !targetEvidence.symbol_addresses.isEmpty() ||
                !targetEvidence.functions.isEmpty();
            evidence.targets.add(targetEvidence);
        }

        decompiler.dispose();
        evidence.targets.sort(
            Comparator.comparing(value -> value.target)
        );
        Collections.sort(evidence.warnings);

        Gson gson = new GsonBuilder()
            .disableHtmlEscaping()
            .setPrettyPrinting()
            .create();
        try (Writer writer = new FileWriter(outputPath)) {
            gson.toJson(evidence, writer);
            writer.write("\n");
        }
        println(
            "[ExportNpuAbi] wrote " + outputPath +
            " targets=" + evidence.targets.size()
        );
    }

    private FunctionEvidence exportFunction(
        Function function,
        String role,
        FunctionManager functionManager,
        Listing listing,
        ReferenceManager referenceManager,
        DecompInterface decompiler,
        List<String> diagnosticFilters
    ) {
        FunctionEvidence result = new FunctionEvidence();
        result.role = role;
        result.name = function.getName();
        result.qualified_name = function.getName(true);
        result.entry = function.getEntryPoint().toString();
        result.body_bytes = function.getBody().getNumAddresses();
        result.external = function.isExternal();
        result.thunk = function.isThunk();

        ReferenceIterator incoming = referenceManager.getReferencesTo(
            function.getEntryPoint()
        );
        while (incoming.hasNext() && !monitor.isCancelled()) {
            Reference reference = incoming.next();
            if (!reference.getReferenceType().isCall()) {
                continue;
            }
            Function caller = functionManager.getFunctionContaining(
                reference.getFromAddress()
            );
            result.callers.add(
                new Edge(
                    reference.getFromAddress().toString(),
                    caller == null ? "<unresolved>" : caller.getName(true),
                    caller == null
                        ? null
                        : caller.getEntryPoint().toString(),
                    reference.getReferenceType().getName()
                )
            );
        }

        if (!function.isExternal()) {
            InstructionIterator instructions = listing.getInstructions(
                function.getBody(), true
            );
            while (instructions.hasNext() && !monitor.isCancelled()) {
                Instruction instruction = instructions.next();
                for (Reference reference : instruction.getReferencesFrom()) {
                    if (reference.getReferenceType().isCall()) {
                        Function callee = functionManager.getFunctionAt(
                            reference.getToAddress()
                        );
                        result.callees.add(
                            new Edge(
                                instruction.getAddress().toString(),
                                callee == null
                                    ? symbolOrUnresolved(
                                        reference.getToAddress()
                                    )
                                    : callee.getName(true),
                                callee == null
                                    ? reference.getToAddress().toString()
                                    : callee.getEntryPoint().toString(),
                                reference.getReferenceType().getName()
                            )
                        );
                    }

                    if (!diagnosticFilters.isEmpty()) {
                        Data data = listing.getDataAt(
                            reference.getToAddress()
                        );
                        if (data == null) {
                            data = listing.getDataContaining(
                                reference.getToAddress()
                            );
                        }
                        if (data == null || !(data.getValue() instanceof String)) {
                            continue;
                        }
                        String value = (String)data.getValue();
                        if (!matchesAny(value, diagnosticFilters)) {
                            continue;
                        }
                        result.diagnostics.add(
                            new Diagnostic(
                                instruction.getAddress().toString(),
                                reference.getToAddress().toString(),
                                bounded(value, MAX_DIAGNOSTIC_CHARS)
                            )
                        );
                    }
                }
            }
        }

        result.callers = uniqueSortedEdges(result.callers);
        result.callees = uniqueSortedEdges(result.callees);
        result.diagnostics = uniqueSortedDiagnostics(result.diagnostics);

        if (function.isExternal()) {
            result.decompile_success = false;
            result.decompile_error = "external function";
            result.decompiled_c = null;
        }
        else {
            try {
                DecompileResults decompiled = decompiler.decompileFunction(
                    function, DECOMPILE_TIMEOUT_SECONDS, monitor
                );
                if (
                    decompiled != null &&
                    decompiled.decompileCompleted() &&
                    decompiled.getDecompiledFunction() != null
                ) {
                    String code = decompiled
                        .getDecompiledFunction()
                        .getC()
                        .replace("\r\n", "\n")
                        .replace('\r', '\n');
                    result.decompile_success = true;
                    result.decompile_truncated =
                        code.length() > MAX_DECOMPILED_CHARS;
                    result.decompiled_c = bounded(
                        code, MAX_DECOMPILED_CHARS
                    );
                }
                else {
                    result.decompile_success = false;
                    result.decompile_error =
                        decompiled == null
                            ? "no decompiler result"
                            : bounded(
                                decompiled.getErrorMessage(), 1000
                            );
                }
            }
            catch (Exception exception) {
                result.decompile_success = false;
                result.decompile_error = bounded(
                    exception.getClass().getSimpleName() +
                    ": " + exception.getMessage(),
                    1000
                );
            }
        }
        return result;
    }

    private String symbolOrUnresolved(Address address) {
        Symbol symbol = currentProgram
            .getSymbolTable()
            .getPrimarySymbol(address);
        return symbol == null ? "<unresolved>" : symbol.getName(true);
    }

    private static List<String> splitAndSort(String value) {
        Set<String> unique = new LinkedHashSet<>();
        for (String item : value.split(",")) {
            String normalized = item.trim();
            if (!normalized.isEmpty()) {
                unique.add(normalized);
            }
        }
        List<String> result = new ArrayList<>(unique);
        Collections.sort(result);
        return result;
    }

    private static boolean matchesAny(
        String value, List<String> fragments
    ) {
        String folded = value.toLowerCase(Locale.ROOT);
        for (String fragment : fragments) {
            if (folded.contains(fragment.toLowerCase(Locale.ROOT))) {
                return true;
            }
        }
        return false;
    }

    private static String bounded(String value, int maximum) {
        if (value == null || value.length() <= maximum) {
            return value;
        }
        int head = maximum / 2;
        int tail = maximum - head;
        return value.substring(0, head) +
            "\n/* ... truncated ... */\n" +
            value.substring(value.length() - tail);
    }

    private static List<Edge> uniqueSortedEdges(List<Edge> values) {
        Map<String, Edge> unique = new LinkedHashMap<>();
        values.sort(
            Comparator
                .comparing((Edge value) -> value.callsite)
                .thenComparing(value -> value.function)
                .thenComparing(value -> value.address == null ? "" : value.address)
        );
        for (Edge value : values) {
            String key = value.callsite + "\u0000" +
                value.function + "\u0000" + value.address;
            unique.putIfAbsent(key, value);
        }
        return new ArrayList<>(unique.values());
    }

    private static List<Diagnostic> uniqueSortedDiagnostics(
        List<Diagnostic> values
    ) {
        Map<String, Diagnostic> unique = new LinkedHashMap<>();
        values.sort(
            Comparator
                .comparing((Diagnostic value) -> value.from)
                .thenComparing(value -> value.to)
                .thenComparing(value -> value.value)
        );
        for (Diagnostic value : values) {
            String key = value.from + "\u0000" + value.to + "\u0000" +
                value.value;
            unique.putIfAbsent(key, value);
        }
        return new ArrayList<>(unique.values());
    }
}
