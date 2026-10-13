from __future__ import annotations

import copy
import json
import re
import struct
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

NPU_DIRECTORY = Path(__file__).resolve().parents[1]
if str(NPU_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(NPU_DIRECTORY))

import npu_erf_execution_lab as execution
import npu_op_dtype_lab


def fp16_policy_and_document():
    root = '/tmp/wave-npu-exec16a'
    document = json.loads(execution.POC7_POLICY.manifest_path.read_text())
    executable = root + '/bin/wave-npu-execute-fp16-conv-v1'
    wrapper = root + '/lib/libtim-vx.so'
    model = root + '/model/wave-fp16-conv.nbg'
    document['environment']['LD_LIBRARY_PATH'] = root + '/lib:/usr/lib'
    document['remote_executable'] = executable
    document['required_artifacts'] = {
        path: digest for path, digest in document['required_artifacts'].items()
        if not path.startswith('/tmp/')
    }
    document['required_artifacts'].update({executable: '1' * 64, wrapper: '2' * 64, model: '3' * 64})
    absent = ('/lib/libCLC.so', '/usr/lib/libCLC.so', root + '/lib/libCLC.so')
    document['absent_artifacts'] = list(absent)
    input_bytes = struct.pack('<16H', *([0x3c00, 0xbc00] * 8))
    output_bytes = npu_op_dtype_lab.fp16_fir_oracle()
    document['expected_result'].update(input=list(input_bytes), output=list(output_bytes), bytes=32)
    pattern = (
        r'^EXECUTION status=PASS mode=execute-once target=wave operation=fp16-conv '
        r'iterations=1 input_bytes=32 output_bytes=32 graph_us=(?P<graph_us>[0-9]+) '
        r'output_hex=(?P<output_hex>' + output_bytes.hex() + r') cleanup=0$'
    )
    policy = replace(execution.POC7_POLICY, poc_id='16C', experiment='fp16-conv-execute-once',
                     remote_root=root, remote_executable=executable, remote_tim_vx=wrapper,
                     remote_nbg=model, fixed_artifacts=document['required_artifacts'],
                     absent_artifacts=absent, staged_modes={executable: '-r-xr-xr-x', wrapper: '-r--r--r--', model: '-r--r--r--'},
                     nbg_bytes=1800, input_values=tuple(input_bytes), output_values=tuple(output_bytes),
                     execution_re=re.compile(pattern))
    return policy, document


def load(document, policy):
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / 'manifest.json'
        path.write_text(json.dumps(document))
        return execution.load_manifest(path, policy=policy)


class OpDtypeExecutionTests(unittest.TestCase):
    def test_fp16_execution_accepts_exact_32_byte_oracle(self):
        policy, document = fp16_policy_and_document()
        manifest = load(document, policy)
        identity = 'IDENTITY ' + ' '.join(f'{key}={value}' for key, value in execution.EXPECTED_IDENTITY.items())
        line = ('EXECUTION status=PASS mode=execute-once target=wave operation=fp16-conv '
                'iterations=1 input_bytes=32 output_bytes=32 graph_us=123 '
                'output_hex=' + bytes(policy.output_values).hex() + ' cleanup=0')
        result = execution.parse_result(identity + '\n' + line, manifest)
        self.assertEqual(result['bytes'], 32)
        self.assertEqual(result['output'], list(npu_op_dtype_lab.fp16_fir_oracle()))
        self.assertEqual(result['graph_us'], 123)
        self.assertEqual(result['output_hex'], bytes(policy.output_values).hex())
        for invalid in (line.replace('iterations=1', 'iterations=2'),
                        line.replace('cleanup=0', 'cleanup=1'),
                        line.replace('output_hex=0034', 'output_hex=0134')):
            with self.assertRaises(execution.ExecutionResultError):
                execution.parse_result(identity + '\n' + invalid, manifest)

    def test_model_hash_or_manifest_oracle_changes_refused(self):
        policy, document = fp16_policy_and_document()
        for mutate in (
            lambda doc: doc['required_artifacts'].update({policy.remote_nbg: '4' * 64}),
            lambda doc: doc['expected_result'].update(bytes=4),
            lambda doc: doc['expected_result']['output'].__setitem__(0, 1),
            lambda doc: doc.update(remote_argument='--execute-twice'),
        ):
            changed = copy.deepcopy(document)
            mutate(changed)
            with self.assertRaises(execution.ExecutionManifestError):
                load(changed, policy)

    def test_execution_health_detects_service_respawn(self):
        policy, document = fp16_policy_and_document()
        manifest = load(document, policy)
        before = {'processes': ['2197 root /data/waved/waved'], 'dmesg_tail': ['[1.0] baseline']}
        after = copy.deepcopy(before)
        after['processes'] = ['2200 root /data/waved/waved']
        differences, _ = execution.compare_health(manifest, before, after)
        self.assertIn('service process identities changed during execution', differences)


if __name__ == '__main__':
    unittest.main()
