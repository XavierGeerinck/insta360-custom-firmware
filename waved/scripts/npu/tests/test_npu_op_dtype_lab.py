from __future__ import annotations

import copy
import json
import re
import shlex
import struct
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_compile_lab
import npu_op_dtype_lab
import npu_op_dtype_stage


def document_for(policy):
    document = json.loads(npu_compile_lab.CONV_AUDIO_DENOISER_POLICY.manifest_path.read_text())
    document['environment'] = policy.expected_environment
    document['remote_executable'] = policy.remote_executable
    document['candidates'] = dict(policy.candidates)
    document['required_artifacts'] = {
        key: value for key, value in document['required_artifacts'].items()
        if not key.startswith('/tmp/')
    }
    document['required_artifacts'][policy.remote_executable] = '1' * 64
    document['required_artifacts'][policy.remote_root + '/lib/libtim-vx.so'] = '2' * 64
    return document


def load_document(document, policy):
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / 'manifest.json'
        path.write_text(json.dumps(document))
        return npu_compile_lab.load_manifest(path, policy=policy)


class OpDtypeTests(unittest.TestCase):
    def test_diagnostic_variables_reach_remote_process(self):
        for policy in npu_op_dtype_lab.POLICIES.values():
            with self.subTest(operation=policy.operation):
                manifest = load_document(document_for(policy), policy)
                command = npu_compile_lab.build_remote_command(
                    manifest, manifest.candidates['candidate-1'],
                    '__WAVE_COMPILE_EXIT_' + 'a' * 32 + '__',
                )
                tokens = shlex.split(command.splitlines()[0])
                self.assertIn('VIV_VX_DEBUG_LEVEL=1', tokens)
                self.assertIn('VIV_VX_ENABLE_PRINT_TARGET=1', tokens)
                self.assertNotIn('LD_PRELOAD', command)

    def test_changed_environment_or_extra_candidate_refused(self):
        for policy in npu_op_dtype_lab.POLICIES.values():
            for mutate in (
                lambda doc: doc['environment'].update(VIV_VX_ENABLE_PRINT_TARGET='0'),
                lambda doc: doc['environment'].update(LD_PRELOAD='/tmp/hook.so'),
                lambda doc: doc['candidates'].update(extra=doc['candidates']['candidate-1']),
            ):
                document = document_for(policy)
                mutate(document)
                with self.assertRaises(npu_compile_lab.ManifestError):
                    load_document(document, policy)

    def test_wrong_tensor_contract_refused(self):
        for policy in npu_op_dtype_lab.POLICIES.values():
            manifest = load_document(document_for(policy), policy)
            def tensor(expectation):
                return {
                    'dimensions': list(expectation.dimensions),
                    'data_format_name': expectation.data_format_name,
                    'byte_size': expectation.byte_size,
                    'quantization': {
                        'format_name': expectation.quantization_formats[0],
                        'scale': expectation.quantization_scale,
                        'zero_point': expectation.quantization_zero_point,
                    },
                }
            report = {
                'magic': 'VPMN', 'version_hex': '0x00020000',
                'product_hex': '0x1000000e', 'size': 4096,
                'inputs': [tensor(policy.inputs[0])],
                'outputs': [tensor(policy.outputs[0])],
            }
            npu_compile_lab.validate_nbg_report(report, manifest)
            for key, value in [('dimensions', [999]), ('data_format_name', 'fp32'), ('byte_size', 0)]:
                wrong = copy.deepcopy(report)
                wrong['outputs'][0][key] = value
                with self.assertRaises(npu_compile_lab.ResultError):
                    npu_compile_lab.validate_nbg_report(wrong, manifest)

    def test_preflight_blocks_holder_changed_hash_output_or_clc(self):
        for policy in npu_op_dtype_lab.POLICIES.values():
            manifest = load_document(document_for(policy), policy)
            candidate = manifest.candidates['candidate-1']
            snapshot = {
                'serials': [manifest.serial], 'kernel': manifest.kernel,
                'boot_id': '0b8091ee-557d-4857-8019-af1aaab9d665',
                'galcore_device': 'crw-rw---- 1 0 0 199, 0 /dev/galcore',
                'npu_parameters': dict(manifest.npu_parameters),
                'galcore_holders': [], 'artifact_hashes': dict(manifest.required_artifacts),
                'absent_artifact_states': {path: {'status': 'absent'} for path in policy.absent_artifacts},
                'output_states': {path: {'status': 'absent'} for path in [candidate.output, candidate.partial_output]},
                'dmesg_tail': ['[1.0] baseline'], 'errors': [],
            }
            npu_compile_lab.verify_preflight(manifest, candidate, snapshot)
            for mutate in (
                lambda state: state.update(galcore_holders=[{'pid': 42, 'fd': 3}]),
                lambda state: state['artifact_hashes'].update({manifest.remote_executable: '3' * 64}),
                lambda state: state['output_states'].update({candidate.output: {'status': 'present'}}),
                lambda state: state['absent_artifact_states'].update({policy.absent_artifacts[0]: {'status': 'present'}}),
            ):
                wrong = copy.deepcopy(snapshot)
                mutate(wrong)
                with self.assertRaises(npu_compile_lab.PreflightError):
                    npu_compile_lab.verify_preflight(manifest, candidate, wrong)

    def test_fp16_oracle_boundary_and_constant_signal(self):
        self.assertEqual(
            npu_op_dtype_lab.fp16_fir_oracle(),
            struct.pack('<16H', 0x3400, *([0] * 14), 0xb400),
        )
        self.assertEqual(
            struct.unpack('<16e', npu_op_dtype_lab.fp16_fir_oracle([1.0] * 16)),
            (0.75, *([1.0] * 14), 0.75),
        )
        with self.assertRaises(ValueError):
            npu_op_dtype_lab.fp16_fir_oracle([0.0] * 15)

    def test_fc_oracle_column_layout(self):
        self.assertEqual(npu_op_dtype_lab.u8_fc_oracle(), bytes([0, 5, 10, 15]))
        self.assertEqual(npu_op_dtype_lab.u8_fc_oracle(range(20, 36)), bytes([20, 25, 30, 35]))
        self.assertEqual(npu_op_dtype_lab.u8_fc_oracle([255] * 16), bytes([255] * 4))
        for samples in ([0] * 15, [256] * 16, [True] * 16):
            with self.assertRaises(ValueError):
                npu_op_dtype_lab.u8_fc_oracle(samples)

    def test_wrong_local_hash_refused_before_device_writes(self):
        policy = npu_op_dtype_lab.POLICIES['fp16-conv']
        manifest = load_document(document_for(policy), policy)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'compiler'
            path.write_bytes(b'unexpected payload')
            files = {manifest.remote_executable: (path, '0555')}
            with mock.patch.object(npu_op_dtype_stage, 'local_files', return_value=files), \
                    mock.patch.object(npu_op_dtype_stage, '_adb') as adb:
                with self.assertRaisesRegex(npu_compile_lab.PreflightError, 'hash mismatch'):
                    npu_op_dtype_stage.stage(manifest)
                adb.assert_not_called()

    def test_legacy_shell_failure_is_not_success(self):
        import subprocess
        policy = npu_op_dtype_lab.POLICIES['fp16-conv']
        manifest = load_document(document_for(policy), policy)
        def failing_shell(command, **kwargs):
            sentinel = re.search(r'__WAVE_COMPILE_EXIT_[0-9a-f]{32}__', command[-1]).group()
            return subprocess.CompletedProcess(command, 0, f'{sentinel}:9\n', '')
        with mock.patch.object(npu_op_dtype_stage.subprocess, 'run', side_effect=failing_shell):
            with self.assertRaisesRegex(npu_compile_lab.PreflightError, 'staging shell exited 9'):
                npu_op_dtype_stage._adb(manifest, 'shell', 'false')

    def test_cleanup_refuses_signal_timeout_or_missing_exit_before_device_calls(self):
        policy = npu_op_dtype_lab.POLICIES['fp16-conv']
        manifest = load_document(document_for(policy), policy)
        for exit_code in (None, 124, 134, 139):
            with tempfile.TemporaryDirectory() as temporary:
                run = Path(temporary) / 'negative'
                run.mkdir()
                (run / 'run.json').write_text(json.dumps({
                    'manifest_sha256': manifest.sha256, 'health_differences': [],
                    'transport': {'timed_out': False, 'remote_exit': exit_code},
                }))
                with mock.patch.object(npu_compile_lab, 'DEFAULT_RUN_ROOT', Path(temporary)), \
                        mock.patch.object(npu_op_dtype_stage, '_adb') as adb:
                    with self.assertRaisesRegex(npu_compile_lab.PreflightError, 'normal completion'):
                        npu_op_dtype_stage.cleanup(manifest, 'negative')
                    adb.assert_not_called()

    def test_expanded_cleanup_window_keeps_alert_detection(self):
        previous = {'dmesg_tail': ['[1.0] original marker']}
        current = {'dmesg_tail': ['[3.0] wifi scan'], 'errors': []}
        with mock.patch.object(npu_compile_lab, '_shell_value', return_value='[1.0] original marker \n[2.0] galcore MMU fault\n[3.0] wifi scan'):
            npu_op_dtype_stage.align_cleanup_diagnostics(None, previous, current)
        lines, aligned = npu_compile_lab._new_dmesg_lines(previous['dmesg_tail'], current['dmesg_tail'])
        self.assertTrue(aligned)
        self.assertTrue(any(npu_compile_lab.KERNEL_ALERT_RE.search(line) for line in lines))
        current = {'dmesg_tail': ['[3.0] wifi scan'], 'errors': []}
        with mock.patch.object(npu_compile_lab, '_shell_value', return_value='[3.0] wifi scan'):
            npu_op_dtype_stage.align_cleanup_diagnostics(None, previous, current)
        self.assertFalse(npu_compile_lab._new_dmesg_lines(previous['dmesg_tail'], current['dmesg_tail'])[1])


if __name__ == '__main__':
    unittest.main()
