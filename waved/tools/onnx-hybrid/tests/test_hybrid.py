from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import pytest
from onnx import TensorProto, helper, numpy_helper

from wave_onnx_hybrid.audit import (
    AdmissionLimits,
    AuditPolicy,
    audit_model_bytes,
    audit_model_path,
    canonical_json,
    load_policy,
)
from wave_onnx_hybrid.cli import main as cli_main
from wave_onnx_hybrid.errors import ErrorCode, HybridError
from wave_onnx_hybrid.fixtures import (
    build_fixture_models,
    deterministic_npz_bytes,
    generate_fixtures,
    serialize_model,
    spectral_mask_input,
)
from wave_onnx_hybrid.runtime import (
    ExecutionMode,
    construct_session,
    execute_cpu_model,
)


def cpu_policy(**limit_overrides: int) -> AuditPolicy:
    limits = AdmissionLimits(**limit_overrides)
    return AuditPolicy(
        policy_id="test-cpu-only",
        policy_sha256="1" * 64,
        compiler_manifest_sha256="2" * 64,
        compiler_qualified=False,
        approved_npu_signatures=frozenset(),
        limits=limits,
    )


def spectral_payload() -> bytes:
    return serialize_model(build_fixture_models()["spectral_mask_conv.onnx"])


def audit_spectral():
    return audit_model_bytes(
        spectral_payload(),
        source_name="spectral_mask_conv.onnx",
        policy=cpu_policy(),
    )


def nested_if_model():
    then_graph = helper.make_graph(
        [
            helper.make_node(
                "Identity",
                ["x"],
                ["then_y"],
                name="then.identity",
            )
        ],
        "then-graph",
        [],
        [helper.make_tensor_value_info("then_y", TensorProto.FLOAT, [1])],
    )
    else_graph = helper.make_graph(
        [
            helper.make_node(
                "Identity",
                ["x"],
                ["else_y"],
                name="else.identity",
            )
        ],
        "else-graph",
        [],
        [helper.make_tensor_value_info("else_y", TensorProto.FLOAT, [1])],
    )
    return helper.make_model(
        helper.make_graph(
            [
                helper.make_node(
                    "If",
                    ["condition"],
                    ["y"],
                    name="select",
                    then_branch=then_graph,
                    else_branch=else_graph,
                )
            ],
            "nested-control-flow",
            [
                helper.make_tensor_value_info("condition", TensorProto.BOOL, []),
                helper.make_tensor_value_info("x", TensorProto.FLOAT, [1]),
            ],
            [helper.make_tensor_value_info("y", TensorProto.FLOAT, [1])],
        ),
        opset_imports=[helper.make_opsetid("", 18)],
        ir_version=10,
    )


def write_cpu_policy(root: Path) -> Path:
    compiler_manifest = {
        "schema_version": 1,
        "decision": "parser-shaped-candidate-not-compiler-qualified",
        "authorization": {
            "compiler_execution": False,
            "graph_construction": False,
            "device_execution": False,
        },
    }
    compiler_path = root / "compiler.json"
    compiler_path.write_text(
        json.dumps(compiler_manifest, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    policy_document = {
        "schema_version": 1,
        "policy_id": "test-policy",
        "compiler_manifest": {
            "path": "compiler.json",
            "sha256": hashlib.sha256(compiler_path.read_bytes()).hexdigest(),
        },
        "compiler_qualified": False,
        "approved_npu_signatures": [],
        "limits": AdmissionLimits().to_document(),
    }
    policy_path = root / "policy.json"
    policy_path.write_text(
        json.dumps(policy_document, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return policy_path


class TestFixtures:
    def test_fixture_serialization_is_byte_deterministic(self) -> None:
        first = {
            name: serialize_model(model)
            for name, model in build_fixture_models().items()
        }
        second = {
            name: serialize_model(model)
            for name, model in build_fixture_models().items()
        }
        assert first == second
        assert tuple(first) == (
            "dynamic_identity.onnx",
            "mixed_sin.onnx",
            "spectral_mask_conv.onnx",
        )

    def test_fixture_generation_refuses_overwrite_and_verifies_hashes(
        self, tmp_path: Path
    ) -> None:
        manifest = generate_fixtures(tmp_path)
        assert manifest["schema_version"] == 1
        assert len(manifest["models"]) == 3
        for record in manifest["models"]:
            payload = (tmp_path / record["path"]).read_bytes()
            assert hashlib.sha256(payload).hexdigest() == record["sha256"]
        with pytest.raises(HybridError, match="already exists"):
            generate_fixtures(tmp_path)

    def test_weight_change_changes_payload_and_graph_signature(self) -> None:
        model = build_fixture_models()["spectral_mask_conv.onnx"]
        baseline = audit_model_bytes(
            serialize_model(model),
            source_name="baseline.onnx",
            policy=cpu_policy(),
        )
        changed = copy.deepcopy(model)
        weight = numpy_helper.to_array(changed.graph.initializer[0]).copy()
        weight.flat[0] += np.float32(0.125)
        changed.graph.initializer[0].CopyFrom(
            numpy_helper.from_array(weight, name="conv.weight")
        )
        modified = audit_model_bytes(
            serialize_model(changed),
            source_name="changed.onnx",
            policy=cpu_policy(),
        )
        assert baseline.graph_signature != modified.graph_signature
        assert baseline.model_sha256 != modified.model_sha256

    def test_producer_and_doc_metadata_do_not_change_graph_signature(self) -> None:
        model = build_fixture_models()["spectral_mask_conv.onnx"]
        baseline = audit_model_bytes(
            serialize_model(model),
            source_name="baseline.onnx",
            policy=cpu_policy(),
        )
        changed = copy.deepcopy(model)
        changed.producer_name = "irrelevant-producer"
        changed.producer_version = "99"
        changed.doc_string = "not executable graph state"
        changed.graph.doc_string = "also excluded"
        modified = audit_model_bytes(
            serialize_model(changed),
            source_name="metadata.onnx",
            policy=cpu_policy(),
        )
        assert baseline.graph_signature == modified.graph_signature
        assert baseline.model_sha256 != modified.model_sha256

    def test_nested_graph_doc_metadata_does_not_change_graph_signature(self) -> None:
        model = nested_if_model()
        baseline = audit_model_bytes(
            serialize_model(model),
            source_name="baseline.onnx",
            policy=cpu_policy(),
        )
        changed = copy.deepcopy(model)
        branch = next(
            attribute.g
            for attribute in changed.graph.node[0].attribute
            if attribute.name == "then_branch"
        )
        branch.doc_string = "nested graph documentation is not executable"
        branch.node[0].doc_string = "nested node documentation is not executable"
        modified = audit_model_bytes(
            serialize_model(changed),
            source_name="metadata.onnx",
            policy=cpu_policy(),
        )
        assert baseline.graph_signature == modified.graph_signature
        assert baseline.model_sha256 != modified.model_sha256


class TestAdmissionAndAudit:
    def test_spectral_model_is_cpu_admissible_and_never_npu_resident(
        self,
    ) -> None:
        result = audit_spectral()
        assert result.document["summary"] == {
            "cpu_admissible": True,
            "cpu_nodes": 2,
            "cpu_partitions": 1,
            "npu_nodes": 0,
            "npu_partitions": 0,
        }
        assert [node["op_type"] for node in result.document["nodes"]] == [
            "Conv",
            "Sigmoid",
        ]
        for node in result.document["nodes"]:
            assert node["placement"] == "cpu"
            assert "PARTITION_NOT_PREFLIGHTED" in node["reasons"]
            assert "COMPILER_DEPENDENCY_MISMATCH" in node["reasons"]
        assert result.document["partitions"][0]["placement"] == "cpu"

    def test_mixed_sin_reports_operator_reason(self) -> None:
        payload = serialize_model(build_fixture_models()["mixed_sin.onnx"])
        result = audit_model_bytes(
            payload, source_name="mixed_sin.onnx", policy=cpu_policy()
        )
        sin = result.document["nodes"][1]
        assert sin["op_type"] == "Sin"
        assert "ONNX_OPERATOR_UNSUPPORTED" in sin["reasons"]
        assert sin["placement"] == "cpu"

    def test_dynamic_shape_is_cpu_admissible_but_npu_ineligible(self) -> None:
        payload = serialize_model(build_fixture_models()["dynamic_identity.onnx"])
        result = audit_model_bytes(
            payload,
            source_name="dynamic_identity.onnx",
            policy=cpu_policy(),
        )
        assert result.document["summary"]["cpu_admissible"] is True
        assert "ONNX_SHAPE_UNSUPPORTED" in result.document["nodes"][0]["reasons"]
        assert result.document["inputs"][0]["static"] is False

    def test_nested_graph_inventory_counts_and_bounds_every_subgraph(self) -> None:
        payload = serialize_model(nested_if_model())
        result = audit_model_bytes(
            payload,
            source_name="nested.onnx",
            policy=cpu_policy(),
        )
        assert result.document["graph_inventory"] == {
            "graphs": 3,
            "max_depth": 1,
            "top_level_nodes": 1,
            "nested_nodes": 2,
            "total_nodes": 3,
            "graph_initializer_tensors": 0,
            "attribute_tensors": 0,
            "constant_tensors": 0,
            "constant_tensor_bytes": 0,
        }
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                payload,
                source_name="nested.onnx",
                policy=cpu_policy(max_nodes=2),
            )
        assert captured.value.code is ErrorCode.ONNX_MODEL_LIMIT

    def test_external_tensor_in_nested_graph_is_rejected_before_checker(self) -> None:
        model = nested_if_model()
        branch = next(
            attribute.g
            for attribute in model.graph.node[0].attribute
            if attribute.name == "then_branch"
        )
        tensor = helper.make_tensor("embedded", TensorProto.FLOAT, [1], [1.0])
        tensor.data_location = TensorProto.EXTERNAL
        tensor.external_data.add(key="location", value="../forbidden.bin")
        branch.node[0].CopyFrom(
            helper.make_node(
                "Constant",
                [],
                ["then_y"],
                name="then.constant",
                value=tensor,
            )
        )
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                serialize_model(model),
                source_name="nested-external.onnx",
                policy=cpu_policy(),
            )
        assert captured.value.code is ErrorCode.ONNX_EXTERNAL_DATA

    def test_model_size_limit_fails_before_protobuf_parse(self) -> None:
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                spectral_payload(),
                source_name="too-large.onnx",
                policy=cpu_policy(max_model_bytes=8),
            )
        assert captured.value.code is ErrorCode.ONNX_MODEL_LIMIT

    def test_node_limit_is_enforced(self) -> None:
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                spectral_payload(),
                source_name="too-many-nodes.onnx",
                policy=cpu_policy(max_nodes=1),
            )
        assert captured.value.code is ErrorCode.ONNX_MODEL_LIMIT

    def test_external_initializer_is_rejected(self) -> None:
        model = build_fixture_models()["spectral_mask_conv.onnx"]
        tensor = model.graph.initializer[0]
        tensor.ClearField("raw_data")
        tensor.data_location = TensorProto.EXTERNAL
        entry = tensor.external_data.add()
        entry.key = "location"
        entry.value = "../outside.bin"
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                serialize_model(model),
                source_name="external.onnx",
                policy=cpu_policy(),
            )
        assert captured.value.code is ErrorCode.ONNX_EXTERNAL_DATA

    def test_duplicate_nonempty_node_names_are_rejected(self) -> None:
        model = build_fixture_models()["spectral_mask_conv.onnx"]
        model.graph.node[1].name = model.graph.node[0].name
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                serialize_model(model),
                source_name="duplicate-name.onnx",
                policy=cpu_policy(),
            )
        assert captured.value.code is ErrorCode.ONNX_GRAPH_INVALID

    def test_bad_protobuf_is_rejected_with_stable_code(self) -> None:
        with pytest.raises(HybridError) as captured:
            audit_model_bytes(
                b"this is not ONNX",
                source_name="bad.onnx",
                policy=cpu_policy(),
            )
        assert captured.value.code is ErrorCode.ONNX_PARSE_FAILURE

    def test_audit_path_hashes_exact_file(self, tmp_path: Path) -> None:
        model_path = tmp_path / "model.onnx"
        model_path.write_bytes(spectral_payload())
        result = audit_model_path(model_path, policy=cpu_policy())
        assert (
            result.model_sha256 == hashlib.sha256(model_path.read_bytes()).hexdigest()
        )

    def test_canonical_json_rejects_nonfinite_float(self) -> None:
        with pytest.raises(HybridError) as captured:
            canonical_json({"bad": float("nan")})
        assert captured.value.code is ErrorCode.EVIDENCE_INVALID


class TestPolicy:
    def test_load_policy_binds_refusal_manifest(self, tmp_path: Path) -> None:
        policy_path = write_cpu_policy(tmp_path)
        policy = load_policy(policy_path, repository_root=tmp_path)
        assert policy.compiler_qualified is False
        assert policy.approved_npu_signatures == frozenset()

    def test_policy_cannot_qualify_refused_compiler(self, tmp_path: Path) -> None:
        compiler_path = tmp_path / "compiler.json"
        compiler_path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "authorization": {"compiler_execution": False},
                }
            ),
            encoding="utf-8",
        )
        policy_path = tmp_path / "policy.json"
        policy_path.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "policy_id": "unsafe",
                    "compiler_manifest": {
                        "path": "compiler.json",
                        "sha256": hashlib.sha256(
                            compiler_path.read_bytes()
                        ).hexdigest(),
                    },
                    "compiler_qualified": True,
                    "approved_npu_signatures": ["0" * 64],
                    "limits": AdmissionLimits().to_document(),
                }
            ),
            encoding="utf-8",
        )
        with pytest.raises(HybridError) as captured:
            load_policy(policy_path, repository_root=tmp_path)
        assert captured.value.code is ErrorCode.POLICY_INVALID


class CountingFactory:
    def __init__(self, result=None, error: Exception | None = None) -> None:
        self.calls = 0
        self.result = result if result is not None else object()
        self.error = error

    def __call__(self):
        self.calls += 1
        if self.error is not None:
            raise self.error
        return self.result


class TestSessionFallback:
    def test_hybrid_preflight_fallback_never_calls_npu(self) -> None:
        cpu = CountingFactory()
        npu = CountingFactory(error=RuntimeError("must not be called"))
        selection = construct_session(
            mode=ExecutionMode.HYBRID,
            npu_eligible=False,
            cpu_factory=cpu,
            npu_factory=npu,
        )
        assert selection.classification == "cpu-preflight-fallback"
        assert selection.npu_attempts == 0
        assert selection.cpu_sessions == 1
        assert npu.calls == 0
        assert cpu.calls == 1

    def test_strict_refuses_before_any_session(self) -> None:
        cpu = CountingFactory()
        npu = CountingFactory()
        with pytest.raises(HybridError) as captured:
            construct_session(
                mode=ExecutionMode.STRICT,
                npu_eligible=False,
                cpu_factory=cpu,
                npu_factory=npu,
            )
        assert captured.value.code is ErrorCode.PARTITION_NOT_NPU_RESIDENT
        assert cpu.calls == 0
        assert npu.calls == 0

    def test_hybrid_unexpected_compile_failure_attempts_once_then_cpu(
        self,
    ) -> None:
        cpu = CountingFactory()
        npu = CountingFactory(error=RuntimeError("compiler failed"))
        selection = construct_session(
            mode=ExecutionMode.HYBRID,
            npu_eligible=True,
            cpu_factory=cpu,
            npu_factory=npu,
        )
        assert selection.classification == "cpu-session-construction-fallback"
        assert selection.npu_attempts == 1
        assert selection.cpu_sessions == 1
        assert selection.npu_error == "compiler failed"
        assert npu.calls == 1
        assert cpu.calls == 1

    def test_strict_compile_failure_never_constructs_cpu(self) -> None:
        cpu = CountingFactory()
        npu = CountingFactory(error=RuntimeError("compiler failed"))
        with pytest.raises(HybridError) as captured:
            construct_session(
                mode=ExecutionMode.STRICT,
                npu_eligible=True,
                cpu_factory=cpu,
                npu_factory=npu,
            )
        assert captured.value.code is ErrorCode.PARTITION_COMPILE_FAILURE
        assert npu.calls == 1
        assert cpu.calls == 0


class TestCpuRuntime:
    def write_model(self, tmp_path: Path) -> tuple[Path, object]:
        path = tmp_path / "spectral.onnx"
        path.write_bytes(spectral_payload())
        audit = audit_model_path(path, policy=cpu_policy())
        return path, audit

    def test_cpu_and_hybrid_outputs_match_reference(self, tmp_path: Path) -> None:
        model_path, audit = self.write_model(tmp_path)
        values = np.array(
            [[[-1.0, -0.5, 0.0, 0.5, 1.0, 0.25, -0.25, 0.75]]],
            dtype=np.float32,
        )
        cpu = execute_cpu_model(
            model_path,
            audit,
            {"features": values},
            mode=ExecutionMode.CPU,
        )
        hybrid = execute_cpu_model(
            model_path,
            audit,
            {"features": values},
            mode=ExecutionMode.HYBRID,
        )
        padded = np.pad(values, ((0, 0), (0, 0), (1, 1)))
        conv = (
            padded[:, :, 0:8] * np.float32(0.25)
            + padded[:, :, 1:9] * np.float32(0.5)
            + padded[:, :, 2:10] * np.float32(0.25)
            + np.float32(0.0)
        )
        expected = 1.0 / (1.0 + np.exp(-conv))
        np.testing.assert_allclose(cpu.outputs["mask"], expected, rtol=1e-6, atol=1e-6)
        np.testing.assert_array_equal(cpu.outputs["mask"], hybrid.outputs["mask"])
        assert cpu.evidence["classification"] == "cpu-explicit"
        assert hybrid.evidence["classification"] == "cpu-preflight-fallback"
        assert hybrid.evidence["npu_attempts"] == 0
        assert hybrid.evidence["providers"] == ["CPUExecutionProvider"]

    def test_strict_mode_refuses_without_creating_ort_session(
        self, tmp_path: Path
    ) -> None:
        model_path, audit = self.write_model(tmp_path)
        values = np.zeros((1, 1, 8), dtype=np.float32)
        with pytest.raises(HybridError) as captured:
            execute_cpu_model(
                model_path,
                audit,
                {"features": values},
                mode=ExecutionMode.STRICT,
            )
        assert captured.value.code is ErrorCode.PARTITION_NOT_NPU_RESIDENT

    @pytest.mark.parametrize(
        ("inputs", "code"),
        [
            ({}, ErrorCode.ONNX_INPUT_INVALID),
            (
                {"features": np.zeros((1, 1, 7), dtype=np.float32)},
                ErrorCode.ONNX_INPUT_INVALID,
            ),
            (
                {"features": np.zeros((1, 1, 8), dtype=np.float64)},
                ErrorCode.ONNX_INPUT_INVALID,
            ),
            (
                {
                    "features": np.zeros((1, 1, 8), dtype=np.float32),
                    "extra": np.zeros((1,), dtype=np.float32),
                },
                ErrorCode.ONNX_INPUT_INVALID,
            ),
        ],
    )
    def test_invalid_inputs_fail_before_run(
        self, tmp_path: Path, inputs: dict, code: ErrorCode
    ) -> None:
        model_path, audit = self.write_model(tmp_path)
        with pytest.raises(HybridError) as captured:
            execute_cpu_model(model_path, audit, inputs, mode=ExecutionMode.CPU)
        assert captured.value.code is code

    def test_nonfinite_input_is_rejected(self, tmp_path: Path) -> None:
        model_path, audit = self.write_model(tmp_path)
        values = np.zeros((1, 1, 8), dtype=np.float32)
        values[0, 0, 0] = np.nan
        with pytest.raises(HybridError) as captured:
            execute_cpu_model(
                model_path,
                audit,
                {"features": values},
                mode=ExecutionMode.CPU,
            )
        assert captured.value.code is ErrorCode.ONNX_INPUT_INVALID


class TestCli:
    def prepare(self, tmp_path: Path) -> tuple[Path, Path, Path]:
        model_path = tmp_path / "model.onnx"
        model_path.write_bytes(spectral_payload())
        inputs_path = tmp_path / "inputs.npz"
        inputs_path.write_bytes(
            deterministic_npz_bytes({"features": spectral_mask_input()})
        )
        return model_path, inputs_path, write_cpu_policy(tmp_path)

    def test_audit_cli_writes_once(self, tmp_path: Path) -> None:
        model, _, policy = self.prepare(tmp_path)
        output = tmp_path / "audit.json"
        args = [
            "audit",
            str(model),
            "--output",
            str(output),
            "--policy",
            str(policy),
            "--repository-root",
            str(tmp_path),
        ]
        assert cli_main(args) == 0
        document = json.loads(output.read_text(encoding="utf-8"))
        assert document["summary"]["npu_nodes"] == 0
        assert cli_main(args) == 2

    def test_cpu_and_hybrid_cli_outputs_are_identical(self, tmp_path: Path) -> None:
        model, inputs, policy = self.prepare(tmp_path)
        output_payloads = []
        for mode in ("cpu", "hybrid"):
            outputs = tmp_path / f"{mode}.npz"
            evidence = tmp_path / f"{mode}.json"
            assert (
                cli_main(
                    [
                        "run",
                        str(model),
                        "--inputs",
                        str(inputs),
                        "--mode",
                        mode,
                        "--outputs",
                        str(outputs),
                        "--evidence",
                        str(evidence),
                        "--policy",
                        str(policy),
                        "--repository-root",
                        str(tmp_path),
                    ]
                )
                == 0
            )
            output_payloads.append(outputs.read_bytes())
            run = json.loads(evidence.read_text(encoding="utf-8"))["execution"]
            assert run["npu_attempts"] == 0
        assert output_payloads[0] == output_payloads[1]

    def test_strict_cli_refuses_without_outputs(self, tmp_path: Path) -> None:
        model, inputs, policy = self.prepare(tmp_path)
        outputs = tmp_path / "strict.npz"
        evidence = tmp_path / "strict.json"
        assert (
            cli_main(
                [
                    "run",
                    str(model),
                    "--inputs",
                    str(inputs),
                    "--mode",
                    "strict",
                    "--outputs",
                    str(outputs),
                    "--evidence",
                    str(evidence),
                    "--policy",
                    str(policy),
                    "--repository-root",
                    str(tmp_path),
                ]
            )
            == 2
        )
        assert not outputs.exists()
        assert not evidence.exists()

    def test_fixture_cli_generate_and_verify(self, tmp_path: Path) -> None:
        output = tmp_path / "fixtures"
        assert cli_main(["generate-fixtures", str(output)]) == 0
        assert cli_main(["verify-fixtures", str(output)]) == 0


def test_node_topology_change_changes_graph_signature() -> None:
    first = build_fixture_models()["spectral_mask_conv.onnx"]
    baseline = audit_model_bytes(
        serialize_model(first),
        source_name="baseline.onnx",
        policy=cpu_policy(),
    )
    changed = helper.make_model(
        helper.make_graph(
            [
                helper.make_node(
                    "Conv",
                    ["features", "conv.weight", "conv.bias"],
                    ["conv.out"],
                    name="mask.conv",
                    pads=[1, 1],
                ),
                helper.make_node(
                    "Identity",
                    ["conv.out"],
                    ["mask"],
                    name="mask.identity",
                ),
            ],
            "different-topology",
            [helper.make_tensor_value_info("features", TensorProto.FLOAT, [1, 1, 8])],
            [helper.make_tensor_value_info("mask", TensorProto.FLOAT, [1, 1, 8])],
            initializer=[
                numpy_helper.from_array(
                    np.array([[[0.25, 0.5, 0.25]]], dtype=np.float32),
                    name="conv.weight",
                ),
                numpy_helper.from_array(
                    np.array([0.0], dtype=np.float32), name="conv.bias"
                ),
            ],
        ),
        opset_imports=[helper.make_opsetid("", 18)],
        ir_version=10,
    )
    modified = audit_model_bytes(
        serialize_model(changed),
        source_name="changed.onnx",
        policy=cpu_policy(),
    )
    assert baseline.graph_signature != modified.graph_signature
