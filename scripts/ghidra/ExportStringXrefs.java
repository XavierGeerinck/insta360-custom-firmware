// Headless Ghidra post-script for bounded string-xref/decompiler evidence.
//
// Arguments:
//   0: output JSON path
//   1: comma-separated exact string fragments

import java.io.FileWriter;
import java.io.Writer;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
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
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import ghidra.program.model.symbol.ReferenceManager;

public class ExportStringXrefs extends GhidraScript {
    private static final int SCHEMA_VERSION = 1;
    private static final int DECOMPILE_TIMEOUT_SECONDS = 120;
    private static final int MAX_DECOMPILED_CHARS = 120000;
    private static final int MAX_MATCHES = 128;
    private static final int MAX_REFERENCES_PER_MATCH = 128;

    private static class FunctionEvidence {
        String entry;
        String name;
        String qualified_name;
        long body_bytes;
        boolean decompile_success;
        boolean decompile_truncated;
        String decompile_error;
        String decompiled_c;
    }

    private static class ReferenceEvidence {
        String from;
        String type;
        String function_entry;
        String function_name;
    }

    private static class StringEvidence {
        String address;
        String value;
        List<String> matched_filters = new ArrayList<>();
        List<ReferenceEvidence> references = new ArrayList<>();
    }

    private static class ProgramEvidence {
        int schema_version = SCHEMA_VERSION;
        Map<String, Object> tool = new LinkedHashMap<>();
        Map<String, Object> program = new LinkedHashMap<>();
        List<String> filters = new ArrayList<>();
        List<StringEvidence> strings = new ArrayList<>();
        List<FunctionEvidence> functions = new ArrayList<>();
        List<String> warnings = new ArrayList<>();
    }

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 2) {
            throw new IllegalArgumentException(
                "usage: ExportStringXrefs.java OUTPUT FILTERS"
            );
        }
        String outputPath = args[0];
        List<String> filters = splitAndSort(args[1]);
        if (filters.isEmpty()) {
            throw new IllegalArgumentException("filters must not be empty");
        }

        ProgramEvidence evidence = new ProgramEvidence();
        evidence.filters.addAll(filters);
        evidence.tool.put("name", "Ghidra");
        evidence.tool.put("version", Application.getApplicationVersion());
        evidence.tool.put("script", "ExportStringXrefs.java");
        evidence.program.put("name", currentProgram.getName());
        evidence.program.put(
            "sha256", currentProgram.getExecutableSHA256()
        );
        evidence.program.put(
            "language_id", currentProgram.getLanguageID().toString()
        );
        evidence.program.put(
            "compiler_spec_id",
            currentProgram.getCompilerSpec().getCompilerSpecID().toString()
        );
        evidence.program.put(
            "format", currentProgram.getExecutableFormat()
        );

        FunctionManager functionManager =
            currentProgram.getFunctionManager();
        ReferenceManager referenceManager =
            currentProgram.getReferenceManager();
        Set<Function> referencedFunctions = new LinkedHashSet<>();

        DataIterator dataIterator =
            currentProgram.getListing().getDefinedData(true);
        while (dataIterator.hasNext() && !monitor.isCancelled()) {
            Data data = dataIterator.next();
            Object valueObject = data.getValue();
            if (!(valueObject instanceof String)) {
                continue;
            }
            String value = (String) valueObject;
            List<String> matched = new ArrayList<>();
            for (String filter : filters) {
                if (value.contains(filter)) {
                    matched.add(filter);
                }
            }
            if (matched.isEmpty()) {
                continue;
            }
            if (evidence.strings.size() >= MAX_MATCHES) {
                evidence.warnings.add(
                    "string matches truncated at " + MAX_MATCHES
                );
                break;
            }
            StringEvidence item = new StringEvidence();
            item.address = data.getAddress().toString();
            item.value = value;
            item.matched_filters.addAll(matched);

            ReferenceIterator references =
                referenceManager.getReferencesTo(data.getAddress());
            int referenceCount = 0;
            while (references.hasNext() && !monitor.isCancelled()) {
                Reference reference = references.next();
                if (referenceCount >= MAX_REFERENCES_PER_MATCH) {
                    evidence.warnings.add(
                        "references to " + item.address +
                        " truncated at " + MAX_REFERENCES_PER_MATCH
                    );
                    break;
                }
                ReferenceEvidence referenceEvidence =
                    new ReferenceEvidence();
                referenceEvidence.from =
                    reference.getFromAddress().toString();
                referenceEvidence.type =
                    reference.getReferenceType().toString();
                Function function = functionManager.getFunctionContaining(
                    reference.getFromAddress()
                );
                if (function != null) {
                    referenceEvidence.function_entry =
                        function.getEntryPoint().toString();
                    referenceEvidence.function_name = function.getName(true);
                    if (!function.isExternal()) {
                        referencedFunctions.add(function);
                    }
                }
                item.references.add(referenceEvidence);
                referenceCount++;
            }
            item.references.sort(
                Comparator.comparing(
                    reference -> reference.from
                )
            );
            evidence.strings.add(item);
        }

        DecompInterface decompiler = new DecompInterface();
        decompiler.toggleCCode(true);
        decompiler.toggleSyntaxTree(true);
        if (!decompiler.openProgram(currentProgram)) {
            evidence.warnings.add("decompiler failed to open program");
        }
        List<Function> functions = new ArrayList<>(referencedFunctions);
        functions.sort(
            Comparator.comparing(
                function -> function.getEntryPoint().toString()
            )
        );
        for (Function function : functions) {
            evidence.functions.add(
                exportFunction(function, decompiler)
            );
        }
        decompiler.dispose();

        evidence.strings.sort(
            Comparator.comparing(item -> item.address)
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
    }

    private FunctionEvidence exportFunction(
        Function function,
        DecompInterface decompiler
    ) {
        FunctionEvidence item = new FunctionEvidence();
        item.entry = function.getEntryPoint().toString();
        item.name = function.getName();
        item.qualified_name = function.getName(true);
        item.body_bytes = function.getBody().getNumAddresses();
        DecompileResults results = decompiler.decompileFunction(
            function,
            DECOMPILE_TIMEOUT_SECONDS,
            monitor
        );
        item.decompile_success =
            results != null && results.decompileCompleted();
        if (!item.decompile_success) {
            item.decompile_error =
                results == null
                    ? "decompiler returned null"
                    : results.getErrorMessage();
            return item;
        }
        String decompiled =
            results.getDecompiledFunction().getC();
        if (decompiled.length() > MAX_DECOMPILED_CHARS) {
            item.decompiled_c =
                decompiled.substring(0, MAX_DECOMPILED_CHARS);
            item.decompile_truncated = true;
        } else {
            item.decompiled_c = decompiled;
        }
        return item;
    }

    private List<String> splitAndSort(String value) {
        Set<String> unique = new LinkedHashSet<>();
        for (String item : value.split(",")) {
            String trimmed = item.trim();
            if (!trimmed.isEmpty()) {
                unique.add(trimmed);
            }
        }
        List<String> result = new ArrayList<>(unique);
        Collections.sort(result);
        return result;
    }
}
