# Wave NPU Runtime Firmware Matrix

Generated deterministically by `waved/scripts/npu/abi_inventory.py`.
Function identity uses exact bytes at the ELF physical file offset;
virtual address changes alone do not count as implementation changes.

Reference firmware: `fw_v1.1.6.6`

States: `identical`, `changed`, `added`, `removed`, `absent`,
`size-only`, or `reference-only`. A short function SHA-256 follows
when exact bytes were available.

## `libArchModelSw.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| _No relevant defined symbols_ | — | — | — | — |

## `libGAL.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| _No relevant defined symbols_ | — | — | — | — |

## `libOpenVX.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| _No relevant defined symbols_ | — | — | — | — |

## `libVSC.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| _No relevant defined symbols_ | — | — | — | — |

## `libasr_reverb.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| _No relevant defined symbols_ | — | — | — | — |

## `libmodel_infer.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| `model_infer_aes_destroy` | added `94fd6efe46` | absent | absent | added `94fd6efe46` |
| `model_infer_aes_get_delay` | added `45047e32ec` | absent | absent | added `45047e32ec` |
| `model_infer_aes_get_version` | added `29983d3491` | absent | absent | added `29983d3491` |
| `model_infer_aes_load_from_buf` | added `da7693b7db` | absent | absent | added `357eab711b` |
| `model_infer_aes_load_from_file` | added `2e552aff92` | absent | absent | added `44d9098ffd` |
| `model_infer_aes_load_vdcrn_16k_0_07m_from_buf` | added `7154260180` | absent | absent | added `283d8c36a0` |
| `model_infer_aes_load_vdcrn_16k_0_07m_from_file` | added `e3339c2efa` | absent | absent | added `fa45bb782d` |
| `model_infer_aes_load_vdcrn_16k_0_1m_from_buf` | added `367c71504e` | absent | absent | added `780c1f40b0` |
| `model_infer_aes_load_vdcrn_16k_0_1m_from_file` | added `8c23a3cbf1` | absent | absent | added `1632256082` |
| `model_infer_aes_load_vdcrn_32k_0_1m_from_buf` | added `46c7b7b430` | absent | absent | added `7b8a97f8bc` |
| `model_infer_aes_load_vdcrn_32k_0_1m_from_file` | added `379de4f5e5` | absent | absent | added `2bf02f2941` |
| `model_infer_aes_run` | added `d3e64fa8d9` | absent | absent | added `d3e64fa8d9` |
| `model_infer_ans_destroy` | added `94fd6efe46` | absent | absent | added `94fd6efe46` |
| `model_infer_ans_get_delay` | added `dd049fdb15` | absent | absent | added `dd049fdb15` |
| `model_infer_ans_get_mask` | added `c39d819178` | absent | absent | added `c39d819178` |
| `model_infer_ans_get_post_filter` | added `fd1bd98524` | absent | absent | added `fd1bd98524` |
| `model_infer_ans_get_snr_db` | added `27deb51610` | absent | absent | added `27deb51610` |
| `model_infer_ans_get_strength` | added `80a52e0b07` | absent | absent | added `80a52e0b07` |
| `model_infer_ans_get_version` | added `12905d82c7` | absent | absent | added `12905d82c7` |
| `model_infer_ans_load_from_buf` | added `f7558f0793` | absent | absent | added `bdd86a2bd6` |
| `model_infer_ans_load_from_file` | added `a0b4e36aff` | absent | absent | added `3fd440ced0` |
| `model_infer_ans_load_mulch_fusion_16k_16ms_56m_from_buf` | added `37b42863fa` | absent | absent | added `31ce537d75` |
| `model_infer_ans_load_mulch_fusion_16k_16ms_56m_from_file` | added `7f78e898b4` | absent | absent | added `af019d512f` |
| `model_infer_ans_load_vaibf_32k_380m_from_buf` | added `e34eac1ed2` | absent | absent | added `47d6cca122` |
| `model_infer_ans_load_vaibf_32k_380m_from_file` | added `68ea3da691` | absent | absent | added `0f528438d4` |
| `model_infer_ans_load_vaibf_32k_380m_pipline_from_buf` | absent | absent | absent | added `8ca5ff5aff` |
| `model_infer_ans_load_vaibf_32k_380m_pipline_from_file` | absent | absent | absent | added `f3e0b76f7b` |
| `model_infer_ans_load_vdf2_16k_0_25m_from_buf` | added `1f0c3f23b5` | absent | absent | added `19935a3ff5` |
| `model_infer_ans_load_vdf2_16k_0_25m_from_file` | added `98e79ef86c` | absent | absent | added `5d5e162cee` |
| `model_infer_ans_load_vdf2_16k_0_35m_from_buf` | added `322c3f2330` | absent | absent | added `de45ab9481` |
| `model_infer_ans_load_vdf2_16k_0_35m_from_file` | added `6c12ec2868` | absent | absent | added `40361cc21f` |
| `model_infer_ans_load_vdf2_32k_0_7m_from_buf` | absent | absent | absent | added `2a72b67c02` |
| `model_infer_ans_load_vdf2_32k_0_7m_from_file` | absent | absent | absent | added `14023c6f9d` |
| `model_infer_ans_load_vdf2_32k_0_93m_from_buf` | added `c11f4198ac` | absent | absent | added `18731cd2fd` |
| `model_infer_ans_load_vdf2_32k_0_93m_from_file` | added `ae71b13b6d` | absent | absent | added `bcb0d601a4` |
| `model_infer_ans_load_vdf2_32k_0_96m_from_buf` | added `ada47fe4a1` | absent | absent | added `b528a003a0` |
| `model_infer_ans_load_vdf2_32k_0_96m_from_file` | added `b14034f86c` | absent | absent | added `dbce61e75a` |
| `model_infer_ans_load_vdf2_32k_1_0m_from_buf` | added `bb5d66a277` | absent | absent | added `4791b89f9c` |
| `model_infer_ans_load_vdf2_32k_1_0m_from_file` | added `947b85af3b` | absent | absent | added `9947b4dc37` |
| `model_infer_ans_load_vdf2_32k_1_14m_from_buf` | added `623041a398` | absent | absent | added `253348387f` |
| `model_infer_ans_load_vdf2_32k_1_14m_from_file` | added `16e7d939ce` | absent | absent | added `5af79a34ce` |
| `model_infer_ans_run` | added `99b6393024` | absent | absent | added `99b6393024` |
| `model_infer_ans_set_post_filter` | added `b8bba35739` | absent | absent | added `b8bba35739` |
| `model_infer_ans_set_strength` | added `94fceba991` | absent | absent | added `94fceba991` |

## `libnnsdk.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| `aml_adla_create_network` | identical `5e1642c018` | identical `5e1642c018` | identical `5e1642c018` | identical `5e1642c018` |
| `aml_adla_create_network_common` | identical `4c282aee78` | identical `4c282aee78` | identical `4c282aee78` | identical `4c282aee78` |
| `aml_adla_create_network_off` | identical `5388ff5cb4` | identical `5388ff5cb4` | identical `5388ff5cb4` | identical `5388ff5cb4` |
| `aml_adla_create_network_on` | identical `1d25eaf75b` | identical `1d25eaf75b` | identical `1d25eaf75b` | identical `1d25eaf75b` |
| `aml_adla_destroy` | identical `3e6e33eb42` | identical `3e6e33eb42` | identical `3e6e33eb42` | identical `3e6e33eb42` |
| `aml_adla_destroy_off` | identical `09e9573936` | identical `09e9573936` | identical `09e9573936` | identical `09e9573936` |
| `aml_adla_disableProfile` | identical `cfd70c43b2` | identical `cfd70c43b2` | identical `cfd70c43b2` | identical `cfd70c43b2` |
| `aml_adla_enableProfile` | identical `9d21f4ae0b` | identical `9d21f4ae0b` | identical `9d21f4ae0b` | identical `9d21f4ae0b` |
| `aml_adla_flushBuffer` | identical `eb5830c4fd` | identical `eb5830c4fd` | identical `eb5830c4fd` | identical `eb5830c4fd` |
| `aml_adla_flush_handle` | identical `9c5e936ab9` | identical `9c5e936ab9` | identical `9c5e936ab9` | identical `9c5e936ab9` |
| `aml_adla_freeAlignedBuffer` | identical `59552a5c18` | identical `59552a5c18` | identical `59552a5c18` | identical `59552a5c18` |
| `aml_adla_freeAlignedBuffer_off` | identical `ac3729cca8` | identical `ac3729cca8` | identical `ac3729cca8` | identical `ac3729cca8` |
| `aml_adla_freeBuffer` | identical `dece3e8567` | identical `dece3e8567` | identical `dece3e8567` | identical `dece3e8567` |
| `aml_adla_free_tensor_info` | identical `2c392f4284` | identical `2c392f4284` | identical `2c392f4284` | identical `2c392f4284` |
| `aml_adla_getHardwareStatus` | identical `207d8ce17d` | identical `207d8ce17d` | identical `207d8ce17d` | identical `207d8ce17d` |
| `aml_adla_getIoTensorInfo` | identical `67e2563655` | identical `67e2563655` | identical `67e2563655` | identical `67e2563655` |
| `aml_adla_getProfileInfo` | identical `7f3950395c` | identical `7f3950395c` | identical `7f3950395c` | identical `7f3950395c` |
| `aml_adla_get_chip_info` | identical `2507590f96` | identical `2507590f96` | identical `2507590f96` | identical `2507590f96` |
| `aml_adla_get_input_tensor_info` | identical `1fd196ce43` | identical `1fd196ce43` | identical `1fd196ce43` | identical `1fd196ce43` |
| `aml_adla_get_output_tensor_info` | identical `45de45a72f` | identical `45de45a72f` | identical `45de45a72f` | identical `45de45a72f` |
| `aml_adla_inputs_set` | identical `ec30cf6319` | identical `ec30cf6319` | identical `ec30cf6319` | identical `ec30cf6319` |
| `aml_adla_inputs_set_off` | identical `3a4a3f03c8` | identical `3a4a3f03c8` | identical `3a4a3f03c8` | identical `3a4a3f03c8` |
| `aml_adla_mallocAlignedBuffer` | identical `c3c7876952` | identical `c3c7876952` | identical `c3c7876952` | identical `c3c7876952` |
| `aml_adla_mallocAlignedBuffer_off` | identical `01fab41036` | identical `01fab41036` | identical `01fab41036` | identical `01fab41036` |
| `aml_adla_mallocBuffer` | identical `560e89b684` | identical `560e89b684` | identical `560e89b684` | identical `560e89b684` |
| `aml_adla_outputs_get` | identical `5c85923279` | identical `5c85923279` | identical `5c85923279` | identical `5c85923279` |
| `aml_adla_outputs_get_off` | identical `ec67a31129` | identical `ec67a31129` | identical `ec67a31129` | identical `ec67a31129` |
| `aml_adla_outputs_set` | identical `63178c0a42` | identical `63178c0a42` | identical `63178c0a42` | identical `63178c0a42` |
| `aml_adla_outputs_set_off` | identical `f847c12667` | identical `f847c12667` | identical `f847c12667` | identical `f847c12667` |
| `aml_adla_run_network` | identical `9b3845b271` | identical `9b3845b271` | identical `9b3845b271` | identical `9b3845b271` |
| `aml_adla_run_network_off` | identical `78dcfd5d67` | identical `78dcfd5d67` | identical `78dcfd5d67` | identical `78dcfd5d67` |
| `aml_adla_setAutoSuspend` | identical `2fc7bb2c83` | identical `2fc7bb2c83` | identical `2fc7bb2c83` | identical `2fc7bb2c83` |
| `aml_adla_setKvcacheopt` | identical `15b4e11e67` | identical `15b4e11e67` | identical `15b4e11e67` | identical `15b4e11e67` |
| `aml_adla_setPowerPolicy` | identical `7649958264` | identical `7649958264` | identical `7649958264` | identical `7649958264` |
| `aml_adla_setProfile` | identical `2ac5b9f852` | identical `2ac5b9f852` | identical `2ac5b9f852` | identical `2ac5b9f852` |
| `aml_adla_setfreq` | identical `19f4c247ce` | identical `19f4c247ce` | identical `19f4c247ce` | identical `19f4c247ce` |
| `aml_adla_swapExternalIoBuffer` | identical `7a0a488003` | identical `7a0a488003` | identical `7a0a488003` | identical `7a0a488003` |
| `aml_adla_swapInputBuffer` | identical `d49baf944a` | identical `d49baf944a` | identical `d49baf944a` | identical `d49baf944a` |
| `aml_adla_swapOutputBuffer` | identical `b005608819` | identical `b005608819` | identical `b005608819` | identical `b005608819` |
| `aml_adla_switchInputBuffer` | identical `be2fe78f06` | identical `be2fe78f06` | identical `be2fe78f06` | identical `be2fe78f06` |
| `aml_adla_switchOutputBuffer` | identical `36f3dbfb81` | identical `36f3dbfb81` | identical `36f3dbfb81` | identical `36f3dbfb81` |
| `aml_adla_updateKvcacheinfo` | identical `13deb44ac2` | identical `13deb44ac2` | identical `13deb44ac2` | identical `13deb44ac2` |
| `aml_adla_wait` | identical `15c572275d` | identical `15c572275d` | identical `15c572275d` | identical `15c572275d` |
| `aml_delegate_create_network` | identical `948ce0d6df` | identical `948ce0d6df` | identical `948ce0d6df` | identical `948ce0d6df` |
| `aml_delegate_destroy` | identical `882c06ab5b` | identical `882c06ab5b` | identical `882c06ab5b` | identical `882c06ab5b` |
| `aml_delegate_disableProfile` | identical `3beb0a509b` | identical `3beb0a509b` | identical `3beb0a509b` | identical `3beb0a509b` |
| `aml_delegate_enableProfile` | identical `41702c38d2` | identical `41702c38d2` | identical `41702c38d2` | identical `41702c38d2` |
| `aml_delegate_flushBuffer` | identical `9ac644fbe2` | identical `9ac644fbe2` | identical `9ac644fbe2` | identical `9ac644fbe2` |
| `aml_delegate_flush_handle` | identical `7b2179ac79` | identical `7b2179ac79` | identical `7b2179ac79` | identical `7b2179ac79` |
| `aml_delegate_freeAlignedBuffer` | identical `d6de7ec23f` | identical `d6de7ec23f` | identical `d6de7ec23f` | identical `d6de7ec23f` |
| `aml_delegate_freeBuffer` | identical `c4b364f9e0` | identical `c4b364f9e0` | identical `c4b364f9e0` | identical `c4b364f9e0` |
| `aml_delegate_free_tensor_info` | identical `51090c2813` | identical `51090c2813` | identical `51090c2813` | identical `51090c2813` |
| `aml_delegate_getHardwareStatus` | identical `cfcad5770b` | identical `cfcad5770b` | identical `cfcad5770b` | identical `cfcad5770b` |
| `aml_delegate_getIoTensorInfo` | identical `be546f7c4d` | identical `be546f7c4d` | identical `be546f7c4d` | identical `be546f7c4d` |
| `aml_delegate_getProfileInfo` | identical `51cba3231f` | identical `51cba3231f` | identical `51cba3231f` | identical `51cba3231f` |
| `aml_delegate_get_chip_info` | identical `86c8d6fabb` | identical `86c8d6fabb` | identical `86c8d6fabb` | identical `86c8d6fabb` |
| `aml_delegate_get_input_tensor_info` | identical `2336eb3d5a` | identical `2336eb3d5a` | identical `2336eb3d5a` | identical `2336eb3d5a` |
| `aml_delegate_get_output_tensor_info` | identical `2f5cd9b184` | identical `2f5cd9b184` | identical `2f5cd9b184` | identical `2f5cd9b184` |
| `aml_delegate_inputs_set` | identical `e475045a9b` | identical `e475045a9b` | identical `e475045a9b` | identical `e475045a9b` |
| `aml_delegate_mallocAlignedBuffer` | identical `87d40d8b75` | identical `87d40d8b75` | identical `87d40d8b75` | identical `87d40d8b75` |
| `aml_delegate_mallocBuffer` | identical `4739238205` | identical `4739238205` | identical `4739238205` | identical `4739238205` |
| `aml_delegate_outputs_get` | identical `e41397e968` | identical `e41397e968` | identical `e41397e968` | identical `e41397e968` |
| `aml_delegate_outputs_set` | identical `f27bb3fbce` | identical `f27bb3fbce` | identical `f27bb3fbce` | identical `f27bb3fbce` |
| `aml_delegate_run_network` | identical `db11fed205` | identical `db11fed205` | identical `db11fed205` | identical `db11fed205` |
| `aml_delegate_setAutoSuspend` | identical `2ccb28cbba` | identical `2ccb28cbba` | identical `2ccb28cbba` | identical `2ccb28cbba` |
| `aml_delegate_setKvcacheopt` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_delegate_setPowerPolicy` | identical `3ae1fcf22e` | identical `3ae1fcf22e` | identical `3ae1fcf22e` | identical `3ae1fcf22e` |
| `aml_delegate_setProfile` | identical `d46e31ba40` | identical `d46e31ba40` | identical `d46e31ba40` | identical `d46e31ba40` |
| `aml_delegate_swapExternalIoBuffer` | identical `d9ad716e6d` | identical `d9ad716e6d` | identical `d9ad716e6d` | identical `d9ad716e6d` |
| `aml_delegate_swapInputBuffer` | identical `3d1711c34a` | identical `3d1711c34a` | identical `3d1711c34a` | identical `3d1711c34a` |
| `aml_delegate_swapOutputBuffer` | identical `e954aeb526` | identical `e954aeb526` | identical `e954aeb526` | identical `e954aeb526` |
| `aml_delegate_switchInputBuffer` | identical `e607a73ce6` | identical `e607a73ce6` | identical `e607a73ce6` | identical `e607a73ce6` |
| `aml_delegate_switchOutputBuffer` | identical `3e70f28f51` | identical `3e70f28f51` | identical `3e70f28f51` | identical `3e70f28f51` |
| `aml_delegate_updateKvcacheinfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_module_create` | identical `800a5d8d4e` | identical `800a5d8d4e` | identical `800a5d8d4e` | identical `800a5d8d4e` |
| `aml_module_destroy` | identical `31a0d8b74c` | identical `31a0d8b74c` | identical `31a0d8b74c` | identical `31a0d8b74c` |
| `aml_module_input_set` | identical `4ceeacc26b` | identical `4ceeacc26b` | identical `4ceeacc26b` | identical `4ceeacc26b` |
| `aml_module_output_get` | identical `1aa64d8f6d` | identical `1aa64d8f6d` | identical `1aa64d8f6d` | identical `1aa64d8f6d` |
| `aml_module_output_get_simple` | identical `619212caef` | identical `619212caef` | identical `619212caef` | identical `619212caef` |
| `aml_util_disableProfile` | identical `7f14d149ff` | identical `7f14d149ff` | identical `7f14d149ff` | identical `7f14d149ff` |
| `aml_util_enableProfile` | identical `3f72f0a543` | identical `3f72f0a543` | identical `3f72f0a543` | identical `3f72f0a543` |
| `aml_util_flushBuffer` | identical `f16fcfa5d0` | identical `f16fcfa5d0` | identical `f16fcfa5d0` | identical `f16fcfa5d0` |
| `aml_util_flushTensorHandle` | identical `60e22938aa` | identical `60e22938aa` | identical `60e22938aa` | identical `60e22938aa` |
| `aml_util_freeAlignedBuffer` | identical `be661721d3` | identical `be661721d3` | identical `be661721d3` | identical `be661721d3` |
| `aml_util_freeBuffer` | identical `7c9cdb1c4a` | identical `7c9cdb1c4a` | identical `7c9cdb1c4a` | identical `7c9cdb1c4a` |
| `aml_util_freeTensorInfo` | identical `c7bc9725d0` | identical `c7bc9725d0` | identical `c7bc9725d0` | identical `c7bc9725d0` |
| `aml_util_getHardwareStatus` | identical `16f1a99894` | identical `16f1a99894` | identical `16f1a99894` | identical `16f1a99894` |
| `aml_util_getInputTensorInfo` | identical `b5d7b0aa51` | identical `b5d7b0aa51` | identical `b5d7b0aa51` | identical `b5d7b0aa51` |
| `aml_util_getOutputTensorInfo` | identical `4a723ca255` | identical `4a723ca255` | identical `4a723ca255` | identical `4a723ca255` |
| `aml_util_getProfileInfo` | identical `3c8eb5217e` | identical `3c8eb5217e` | identical `3c8eb5217e` | identical `3c8eb5217e` |
| `aml_util_getTensorInfo` | identical `84208b1a48` | identical `84208b1a48` | identical `84208b1a48` | identical `84208b1a48` |
| `aml_util_mallocAlignedBuffer` | identical `6991baae8e` | identical `6991baae8e` | identical `6991baae8e` | identical `6991baae8e` |
| `aml_util_mallocBuffer` | identical `f6bbb742e7` | identical `f6bbb742e7` | identical `f6bbb742e7` | identical `f6bbb742e7` |
| `aml_util_setAutoSuspend` | identical `22d3f123b1` | identical `22d3f123b1` | identical `22d3f123b1` | identical `22d3f123b1` |
| `aml_util_setKvcacheopt` | identical `735cf7199a` | identical `735cf7199a` | identical `735cf7199a` | identical `735cf7199a` |
| `aml_util_setPowerPolicy` | identical `a67aba84d1` | identical `a67aba84d1` | identical `a67aba84d1` | identical `a67aba84d1` |
| `aml_util_setProfile` | identical `977e93acf8` | identical `977e93acf8` | identical `977e93acf8` | identical `977e93acf8` |
| `aml_util_swapExternalInputBuffer` | identical `cc798ef70e` | identical `cc798ef70e` | identical `cc798ef70e` | identical `cc798ef70e` |
| `aml_util_swapExternalOutputBuffer` | identical `b3c3eb1e51` | identical `b3c3eb1e51` | identical `b3c3eb1e51` | identical `b3c3eb1e51` |
| `aml_util_swapInputBuffer` | identical `02fc62e893` | identical `02fc62e893` | identical `02fc62e893` | identical `02fc62e893` |
| `aml_util_swapOutputBuffer` | identical `63d8412762` | identical `63d8412762` | identical `63d8412762` | identical `63d8412762` |
| `aml_util_switchInputBuffer` | identical `ea3d3a55df` | identical `ea3d3a55df` | identical `ea3d3a55df` | identical `ea3d3a55df` |
| `aml_util_switchOutputBuffer` | identical `1065144412` | identical `1065144412` | identical `1065144412` | identical `1065144412` |
| `aml_util_updateKvcacheinfo` | identical `b9656dc160` | identical `b9656dc160` | identical `b9656dc160` | identical `b9656dc160` |
| `aml_viplite_create_network` | identical `c1ad4e807c` | identical `c1ad4e807c` | identical `c1ad4e807c` | identical `c1ad4e807c` |
| `aml_viplite_create_network_off` | identical `ca8b6c3081` | identical `ca8b6c3081` | identical `ca8b6c3081` | identical `ca8b6c3081` |
| `aml_viplite_destroy` | identical `42e5ebcbde` | identical `42e5ebcbde` | identical `42e5ebcbde` | identical `42e5ebcbde` |
| `aml_viplite_destroy_network` | identical `c9621746b9` | identical `c9621746b9` | identical `c9621746b9` | identical `c9621746b9` |
| `aml_viplite_disableProfile` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_enableProfile` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_flushBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_flush_handle` | identical `34223f6481` | identical `34223f6481` | identical `34223f6481` | identical `34223f6481` |
| `aml_viplite_freeAlignedBuffer` | identical `110f46b5b3` | identical `110f46b5b3` | identical `110f46b5b3` | identical `110f46b5b3` |
| `aml_viplite_freeBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_free_tensor_info` | identical `d499b2d004` | identical `d499b2d004` | identical `d499b2d004` | identical `d499b2d004` |
| `aml_viplite_getContextInfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_getHardwareStatus` | identical `3955612835` | identical `3955612835` | identical `3955612835` | identical `3955612835` |
| `aml_viplite_getIoTensorInfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_getProfileInfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_get_chip_info` | identical `f1bb7707e6` | identical `f1bb7707e6` | identical `f1bb7707e6` | identical `f1bb7707e6` |
| `aml_viplite_get_input_tensor_info` | identical `868a487dc0` | identical `868a487dc0` | identical `868a487dc0` | identical `868a487dc0` |
| `aml_viplite_get_output` | identical `b0c9fd1651` | identical `b0c9fd1651` | identical `b0c9fd1651` | identical `b0c9fd1651` |
| `aml_viplite_get_output_tensor_info` | identical `2819cd48ff` | identical `2819cd48ff` | identical `2819cd48ff` | identical `2819cd48ff` |
| `aml_viplite_init_env` | identical `502ff4fe38` | identical `502ff4fe38` | identical `502ff4fe38` | identical `502ff4fe38` |
| `aml_viplite_inputs_set` | identical `59bfef3f9b` | identical `59bfef3f9b` | identical `59bfef3f9b` | identical `59bfef3f9b` |
| `aml_viplite_mallocAlignedBuffer` | identical `1fc71c0653` | identical `1fc71c0653` | identical `1fc71c0653` | identical `1fc71c0653` |
| `aml_viplite_mallocBuffer` | identical `0b4a084733` | identical `0b4a084733` | identical `0b4a084733` | identical `0b4a084733` |
| `aml_viplite_outputs_get` | identical `a8c5c188c1` | identical `a8c5c188c1` | identical `a8c5c188c1` | identical `a8c5c188c1` |
| `aml_viplite_outputs_set` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_run_network` | identical `eb2c8560cd` | identical `eb2c8560cd` | identical `eb2c8560cd` | identical `eb2c8560cd` |
| `aml_viplite_runnetwork` | identical `524cde433e` | identical `524cde433e` | identical `524cde433e` | identical `524cde433e` |
| `aml_viplite_setAutoSuspend` | identical `34946ee1f8` | identical `34946ee1f8` | identical `34946ee1f8` | identical `34946ee1f8` |
| `aml_viplite_setKvcacheopt` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_setPowerPolicy` | identical `d155d140ff` | identical `d155d140ff` | identical `d155d140ff` | identical `d155d140ff` |
| `aml_viplite_setProfile` | identical `0d20e25c05` | identical `0d20e25c05` | identical `0d20e25c05` | identical `0d20e25c05` |
| `aml_viplite_set_io` | identical `c6648f9049` | identical `c6648f9049` | identical `c6648f9049` | identical `c6648f9049` |
| `aml_viplite_swapExternalIoBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_swapInputBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_swapOutputBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_switchInputBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_switchOutputBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_viplite_updateKvcacheinfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_create_context_ovxlib` | identical `f31a99d4a8` | identical `f31a99d4a8` | identical `f31a99d4a8` | identical `f31a99d4a8` |
| `aml_vipunify_create_graph_ovxlib` | identical `f25f5a2fb1` | identical `f25f5a2fb1` | identical `f25f5a2fb1` | identical `f25f5a2fb1` |
| `aml_vipunify_create_network` | identical `4be6220984` | identical `4be6220984` | identical `4be6220984` | identical `4be6220984` |
| `aml_vipunify_destroy` | identical `4d43135570` | identical `4d43135570` | identical `4d43135570` | identical `4d43135570` |
| `aml_vipunify_destroy_off` | identical `ac5a8cc3e2` | identical `ac5a8cc3e2` | identical `ac5a8cc3e2` | identical `ac5a8cc3e2` |
| `aml_vipunify_destroy_on` | identical `129ad13ac8` | identical `129ad13ac8` | identical `129ad13ac8` | identical `129ad13ac8` |
| `aml_vipunify_destroy_ovxlib` | identical `2b3275aa69` | identical `2b3275aa69` | identical `2b3275aa69` | identical `2b3275aa69` |
| `aml_vipunify_disableProfile` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_enableProfile` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_flushBuffer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_flush_handle` | identical `79982204b2` | identical `79982204b2` | identical `79982204b2` | identical `79982204b2` |
| `aml_vipunify_flush_handle_off` | identical `886e644268` | identical `886e644268` | identical `886e644268` | identical `886e644268` |
| `aml_vipunify_flush_handle_on` | identical `1d9e295b27` | identical `1d9e295b27` | identical `1d9e295b27` | identical `1d9e295b27` |
| `aml_vipunify_flush_handle_ovxlib` | identical `e5bd00a0a1` | identical `e5bd00a0a1` | identical `e5bd00a0a1` | identical `e5bd00a0a1` |
| `aml_vipunify_freeAlignedBuffer` | identical `8cc633c727` | identical `8cc633c727` | identical `8cc633c727` | identical `8cc633c727` |
| `aml_vipunify_freeAlignedBuffer_off` | identical `c035788301` | identical `c035788301` | identical `c035788301` | identical `c035788301` |
| `aml_vipunify_freeAlignedBuffer_on` | identical `8fb2b73d41` | identical `8fb2b73d41` | identical `8fb2b73d41` | identical `8fb2b73d41` |
| `aml_vipunify_freeAlignedBuffer_ovxlib` | identical `e45b231108` | identical `e45b231108` | identical `e45b231108` | identical `e45b231108` |
| `aml_vipunify_freeBuffer` | identical `dd468c6137` | identical `dd468c6137` | identical `dd468c6137` | identical `dd468c6137` |
| `aml_vipunify_free_tensor_info` | identical `2572873a80` | identical `2572873a80` | identical `2572873a80` | identical `2572873a80` |
| `aml_vipunify_free_tensor_info_ovxlib` | identical `df9f7ce8dc` | identical `df9f7ce8dc` | identical `df9f7ce8dc` | identical `df9f7ce8dc` |
| `aml_vipunify_getHardwareStatus` | identical `9131e57755` | identical `9131e57755` | identical `9131e57755` | identical `9131e57755` |
| `aml_vipunify_getProfileInfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_get_chip_info` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_get_input_tensor_info` | identical `7e91d6134c` | identical `7e91d6134c` | identical `7e91d6134c` | identical `7e91d6134c` |
| `aml_vipunify_get_input_tensor_info_off` | identical `42cde25dc1` | identical `42cde25dc1` | identical `42cde25dc1` | identical `42cde25dc1` |
| `aml_vipunify_get_input_tensor_info_on` | identical `251dced00e` | identical `251dced00e` | identical `251dced00e` | identical `251dced00e` |
| `aml_vipunify_get_output_tensor_info` | identical `886b0ea5c0` | identical `886b0ea5c0` | identical `886b0ea5c0` | identical `886b0ea5c0` |
| `aml_vipunify_get_output_tensor_info_off` | identical `8842d16726` | identical `8842d16726` | identical `8842d16726` | identical `8842d16726` |
| `aml_vipunify_get_output_tensor_info_on` | identical `145f74d532` | identical `145f74d532` | identical `145f74d532` | identical `145f74d532` |
| `aml_vipunify_inputs_set` | identical `213bc5654b` | identical `213bc5654b` | identical `213bc5654b` | identical `213bc5654b` |
| `aml_vipunify_inputs_set_off` | identical `a15988006e` | identical `a15988006e` | identical `a15988006e` | identical `a15988006e` |
| `aml_vipunify_inputs_set_on` | identical `f69138d218` | identical `f69138d218` | identical `f69138d218` | identical `f69138d218` |
| `aml_vipunify_inputs_set_ovxlib` | identical `1b28221571` | identical `1b28221571` | identical `1b28221571` | identical `1b28221571` |
| `aml_vipunify_mallocAlignedBuffer` | identical `7c105581a6` | identical `7c105581a6` | identical `7c105581a6` | identical `7c105581a6` |
| `aml_vipunify_mallocAlignedBuffer_off` | identical `6c9982b841` | identical `6c9982b841` | identical `6c9982b841` | identical `6c9982b841` |
| `aml_vipunify_mallocAlignedBuffer_on` | identical `538389a1f9` | identical `538389a1f9` | identical `538389a1f9` | identical `538389a1f9` |
| `aml_vipunify_mallocAlignedBuffer_ovxlib` | identical `d47e2f279e` | identical `d47e2f279e` | identical `d47e2f279e` | identical `d47e2f279e` |
| `aml_vipunify_mallocBuffer` | identical `8a3cdc7363` | identical `8a3cdc7363` | identical `8a3cdc7363` | identical `8a3cdc7363` |
| `aml_vipunify_new_node_ovxlib` | identical `e73b76101a` | identical `e73b76101a` | identical `e73b76101a` | identical `e73b76101a` |
| `aml_vipunify_new_tensor_on_ovxlib` | identical `e67e9cc9d7` | identical `e67e9cc9d7` | identical `e67e9cc9d7` | identical `e67e9cc9d7` |
| `aml_vipunify_new_tensor_ovxlib` | identical `cae3a546c1` | identical `cae3a546c1` | identical `cae3a546c1` | identical `cae3a546c1` |
| `aml_vipunify_outputs_get` | identical `b417435437` | identical `b417435437` | identical `b417435437` | identical `b417435437` |
| `aml_vipunify_outputs_get_off` | identical `61114c8649` | identical `61114c8649` | identical `61114c8649` | identical `61114c8649` |
| `aml_vipunify_outputs_get_on` | identical `115739fb62` | identical `115739fb62` | identical `115739fb62` | identical `115739fb62` |
| `aml_vipunify_outputs_get_ovxlib` | identical `64f61fead9` | identical `64f61fead9` | identical `64f61fead9` | identical `64f61fead9` |
| `aml_vipunify_outputs_set` | identical `7b38ec8328` | identical `7b38ec8328` | identical `7b38ec8328` | identical `7b38ec8328` |
| `aml_vipunify_outputs_set_off` | identical `4430c042d5` | identical `4430c042d5` | identical `4430c042d5` | identical `4430c042d5` |
| `aml_vipunify_outputs_set_on` | identical `861db720c4` | identical `861db720c4` | identical `861db720c4` | identical `861db720c4` |
| `aml_vipunify_outputs_set_ovxlib` | identical `669466afbc` | identical `669466afbc` | identical `669466afbc` | identical `669466afbc` |
| `aml_vipunify_run_network` | identical `c030a22e5e` | identical `c030a22e5e` | identical `c030a22e5e` | identical `c030a22e5e` |
| `aml_vipunify_run_network_off` | identical `61c1f0a9fe` | identical `61c1f0a9fe` | identical `61c1f0a9fe` | identical `61c1f0a9fe` |
| `aml_vipunify_run_network_on` | identical `99ffa6de8a` | identical `99ffa6de8a` | identical `99ffa6de8a` | identical `99ffa6de8a` |
| `aml_vipunify_run_network_ovxlib` | identical `e3e748b794` | identical `e3e748b794` | identical `e3e748b794` | identical `e3e748b794` |
| `aml_vipunify_setAutoSuspend` | identical `bc60a12b5d` | identical `bc60a12b5d` | identical `bc60a12b5d` | identical `bc60a12b5d` |
| `aml_vipunify_setKvcacheopt` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_setPowerPolicy` | identical `7e6a74caa5` | identical `7e6a74caa5` | identical `7e6a74caa5` | identical `7e6a74caa5` |
| `aml_vipunify_setProfile` | identical `bb5b897605` | identical `bb5b897605` | identical `bb5b897605` | identical `bb5b897605` |
| `aml_vipunify_set_graph_inoutputs_ovxlib` | identical `fb9471c8e8` | identical `fb9471c8e8` | identical `fb9471c8e8` | identical `fb9471c8e8` |
| `aml_vipunify_setup_graph_ovxlib` | identical `4dde4630f8` | identical `4dde4630f8` | identical `4dde4630f8` | identical `4dde4630f8` |
| `aml_vipunify_swapExternalIoBuffer` | identical `2a22779c38` | identical `2a22779c38` | identical `2a22779c38` | identical `2a22779c38` |
| `aml_vipunify_swapInputBuffer` | identical `ee5427b8da` | identical `ee5427b8da` | identical `ee5427b8da` | identical `ee5427b8da` |
| `aml_vipunify_swapInputBuffer_off` | identical `f9594ec94a` | identical `f9594ec94a` | identical `f9594ec94a` | identical `f9594ec94a` |
| `aml_vipunify_swapInputBuffer_on` | identical `a15256ba4c` | identical `a15256ba4c` | identical `a15256ba4c` | identical `a15256ba4c` |
| `aml_vipunify_swapOutputBuffer` | identical `85ac243d3e` | identical `85ac243d3e` | identical `85ac243d3e` | identical `85ac243d3e` |
| `aml_vipunify_swapOutputBuffer_off` | identical `18d54fec62` | identical `18d54fec62` | identical `18d54fec62` | identical `18d54fec62` |
| `aml_vipunify_swapOutputBuffer_on` | identical `a672e1ed25` | identical `a672e1ed25` | identical `a672e1ed25` | identical `a672e1ed25` |
| `aml_vipunify_switchInputBuffer` | identical `8631de624c` | identical `8631de624c` | identical `8631de624c` | identical `8631de624c` |
| `aml_vipunify_switchInputBuffer_off` | identical `775cf878b6` | identical `775cf878b6` | identical `775cf878b6` | identical `775cf878b6` |
| `aml_vipunify_switchInputBuffer_on` | identical `4c34b7141d` | identical `4c34b7141d` | identical `4c34b7141d` | identical `4c34b7141d` |
| `aml_vipunify_switchOutputBuffer` | identical `41fb6c3f9f` | identical `41fb6c3f9f` | identical `41fb6c3f9f` | identical `41fb6c3f9f` |
| `aml_vipunify_switchOutputBuffer_off` | identical `3fdb8c61a7` | identical `3fdb8c61a7` | identical `3fdb8c61a7` | identical `3fdb8c61a7` |
| `aml_vipunify_switchOutputBuffer_on` | identical `814f7e7138` | identical `814f7e7138` | identical `814f7e7138` | identical `814f7e7138` |
| `aml_vipunify_switch_buffer_ovxlib` | identical `428dc413f4` | identical `428dc413f4` | identical `428dc413f4` | identical `428dc413f4` |
| `aml_vipunify_updateKvcacheinfo` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `aml_vipunify_verify_graph_ovxlib` | identical `d92c4d6aa4` | identical `d92c4d6aa4` | identical `d92c4d6aa4` | identical `d92c4d6aa4` |
| `aml_vipunify_virtual_tensor_on_ovxlib` | identical `6f7b7d42dc` | identical `6f7b7d42dc` | identical `6f7b7d42dc` | identical `6f7b7d42dc` |
| `get_product_ID` | identical `fc4ff16a59` | identical `fc4ff16a59` | identical `fc4ff16a59` | identical `fc4ff16a59` |
| `read_nbg_binary_file` | identical `b6ec921c51` | identical `b6ec921c51` | identical `b6ec921c51` | identical `b6ec921c51` |
| `read_nbg_from_memory` | identical `cfa710145b` | identical `cfa710145b` | identical `cfa710145b` | identical `cfa710145b` |
| `vipunify_Float16ToFloat32` | identical `53a5b0a701` | identical `53a5b0a701` | identical `53a5b0a701` | identical `53a5b0a701` |
| `vipunify_create_network_off` | identical `0683317217` | identical `0683317217` | identical `0683317217` | identical `0683317217` |
| `vipunify_create_network_on` | identical `b5ad714a39` | identical `b5ad714a39` | identical `b5ad714a39` | identical `b5ad714a39` |
| `vipunify_create_network_on_ovx` | identical `1438610a6b` | identical `1438610a6b` | identical `1438610a6b` | identical `1438610a6b` |
| `vipunify_dtypeToF32` | identical `53aca7f2c9` | identical `53aca7f2c9` | identical `53aca7f2c9` | identical `53aca7f2c9` |
| `vipunify_get_api_format` | identical `aeeb27c144` | identical `aeeb27c144` | identical `aeeb27c144` | identical `aeeb27c144` |
| `vipunify_get_data_format` | identical `024de188d3` | identical `024de188d3` | identical `024de188d3` | identical `024de188d3` |
| `vipunify_load_hw_config` | identical `fa6b5bdb18` | identical `fa6b5bdb18` | identical `fa6b5bdb18` | identical `fa6b5bdb18` |
| `vipunify_set_preprocess` | identical `f72349e93e` | identical `f72349e93e` | identical `f72349e93e` | identical `f72349e93e` |

## `libovxlib.so`

| Symbol | `fw_business_v1.0.2.6` | `fw_v1.0.13.6` | `fw_v1.1.6.6` | `fw_v3.0.0.1` |
|---|---|---|---|---|
| `vsi_nn_Access` | identical `7d3b643151` | identical `7d3b643151` | identical `7d3b643151` | identical `7d3b643151` |
| `vsi_nn_AddBinaryGraphInputsWithCropParam` | identical `9ee3a95e34` | identical `9ee3a95e34` | identical `9ee3a95e34` | identical `9ee3a95e34` |
| `vsi_nn_AddBinaryGraphInputsWithCropParamForCropOnly` | identical `fcb6f4cf8e` | identical `fcb6f4cf8e` | identical `fcb6f4cf8e` | identical `fcb6f4cf8e` |
| `vsi_nn_AddExternalNode` | identical `881b6e1122` | identical `881b6e1122` | identical `881b6e1122` | identical `881b6e1122` |
| `vsi_nn_AddGraphPostProcess` | identical `9acd30df8f` | identical `9acd30df8f` | identical `9acd30df8f` | identical `9acd30df8f` |
| `vsi_nn_AddGraphPreProcess` | identical `b67f5bf21e` | identical `b67f5bf21e` | identical `b67f5bf21e` | identical `b67f5bf21e` |
| `vsi_nn_AddNode` | identical `64580ab39e` | identical `64580ab39e` | identical `64580ab39e` | identical `64580ab39e` |
| `vsi_nn_AddTensor` | identical `bc721a873f` | identical `bc721a873f` | identical `bc721a873f` | identical `bc721a873f` |
| `vsi_nn_AddTensorFromAXISRAM` | identical `5d625a75de` | identical `5d625a75de` | identical `5d625a75de` | identical `5d625a75de` |
| `vsi_nn_AddTensorFromHandle` | identical `969fc5f2c8` | identical `969fc5f2c8` | identical `969fc5f2c8` | identical `969fc5f2c8` |
| `vsi_nn_AddTensorFromView` | identical `0173a749ad` | identical `0173a749ad` | identical `0173a749ad` | identical `0173a749ad` |
| `vsi_nn_AffineToFp32` | identical `3dcab4d5c8` | identical `3dcab4d5c8` | identical `3dcab4d5c8` | identical `3dcab4d5c8` |
| `vsi_nn_AppendNode` | identical `4671318287` | identical `4671318287` | identical `4671318287` | identical `4671318287` |
| `vsi_nn_AsyncRunGraph` | identical `27becd4031` | identical `27becd4031` | identical `27becd4031` | identical `27becd4031` |
| `vsi_nn_AsyncRunWait` | identical `6126515c4d` | identical `6126515c4d` | identical `6126515c4d` | identical `6126515c4d` |
| `vsi_nn_BFp16ToFp32` | identical `0262e22b3b` | identical `0262e22b3b` | identical `0262e22b3b` | identical `0262e22b3b` |
| `vsi_nn_BinaryTreeGetNode` | identical `58ec99132c` | identical `58ec99132c` | identical `58ec99132c` | identical `58ec99132c` |
| `vsi_nn_BinaryTreeNewNode` | identical `070306495c` | identical `070306495c` | identical `070306495c` | identical `070306495c` |
| `vsi_nn_BinaryTreeRemoveNode` | identical `1567624581` | identical `1567624581` | identical `1567624581` | identical `1567624581` |
| `vsi_nn_CMUPose_PostProcess` | identical `3b76cc3053` | identical `3b76cc3053` | identical `3b76cc3053` | identical `3b76cc3053` |
| `vsi_nn_CMUPose_Post_Process` | identical `e07154d04f` | identical `e07154d04f` | identical `e07154d04f` | identical `e07154d04f` |
| `vsi_nn_CalcReshapeTensor` | identical `5308f7a0a2` | identical `5308f7a0a2` | identical `5308f7a0a2` | identical `5308f7a0a2` |
| `vsi_nn_CheckFilePath` | identical `ce4458a184` | identical `ce4458a184` | identical `ce4458a184` | identical `ce4458a184` |
| `vsi_nn_ClientNodePassParameters` | identical `16dd07f39a` | identical `16dd07f39a` | identical `16dd07f39a` | identical `16dd07f39a` |
| `vsi_nn_ComputeFilterSize` | identical `cc38aa332a` | identical `cc38aa332a` | identical `cc38aa332a` | identical `cc38aa332a` |
| `vsi_nn_ComputePadWithPadType` | identical `8aa380ea9f` | identical `8aa380ea9f` | identical `8aa380ea9f` | identical `8aa380ea9f` |
| `vsi_nn_ComputePadWithPadTypeForConv1D` | identical `040eba2b5a` | identical `040eba2b5a` | identical `040eba2b5a` | identical `040eba2b5a` |
| `vsi_nn_ConvertRawTensorToData` | identical `a3b787604a` | identical `a3b787604a` | identical `a3b787604a` | identical `a3b787604a` |
| `vsi_nn_ConvertRawTensorToData2` | identical `5d8df1f67c` | identical `5d8df1f67c` | identical `5d8df1f67c` | identical `5d8df1f67c` |
| `vsi_nn_ConvertTensorToData` | identical `bf77afee79` | identical `bf77afee79` | identical `bf77afee79` | identical `bf77afee79` |
| `vsi_nn_ConvertTensorToFloat32Data` | identical `bea19f15b8` | identical `bea19f15b8` | identical `bea19f15b8` | identical `bea19f15b8` |
| `vsi_nn_CopyDataToTensor` | identical `105d757ef1` | identical `105d757ef1` | identical `105d757ef1` | identical `105d757ef1` |
| `vsi_nn_CopyRawDataToTensor` | identical `14c7687eb9` | identical `14c7687eb9` | identical `14c7687eb9` | identical `14c7687eb9` |
| `vsi_nn_CopyTensorToBuffer` | identical `1064df71e6` | identical `1064df71e6` | identical `1064df71e6` | identical `1064df71e6` |
| `vsi_nn_CopyTensorViaGraphs` | identical `e8da9d5a3a` | identical `e8da9d5a3a` | identical `e8da9d5a3a` | identical `e8da9d5a3a` |
| `vsi_nn_CreateContext` | identical `7a9b03c641` | identical `7a9b03c641` | identical `7a9b03c641` | identical `7a9b03c641` |
| `vsi_nn_CreateGraph` | identical `0774bdcee9` | identical `0774bdcee9` | identical `0774bdcee9` | identical `0774bdcee9` |
| `vsi_nn_CreateNode` | identical `d3a3a5a211` | identical `d3a3a5a211` | identical `d3a3a5a211` | identical `d3a3a5a211` |
| `vsi_nn_CreateTensor` | identical `a0092da7a5` | identical `a0092da7a5` | identical `a0092da7a5` | identical `a0092da7a5` |
| `vsi_nn_CreateTensorFromAXISRAM` | identical `540a5285e8` | identical `540a5285e8` | identical `540a5285e8` | identical `540a5285e8` |
| `vsi_nn_CreateTensorFromData` | identical `ef4ea93cfa` | identical `ef4ea93cfa` | identical `ef4ea93cfa` | identical `ef4ea93cfa` |
| `vsi_nn_CreateTensorFromHandle` | identical `0daf57517b` | identical `0daf57517b` | identical `0daf57517b` | identical `0daf57517b` |
| `vsi_nn_CreateTensorGroup` | identical `960db53b02` | identical `960db53b02` | identical `960db53b02` | identical `960db53b02` |
| `vsi_nn_CreateTensorRelevance` | identical `1a77d5e6cb` | identical `1a77d5e6cb` | identical `1a77d5e6cb` | identical `1a77d5e6cb` |
| `vsi_nn_CreateTensorWithDefault` | identical `6c63a9b7dc` | identical `6c63a9b7dc` | identical `6c63a9b7dc` | identical `6c63a9b7dc` |
| `vsi_nn_CreateViewTensor` | identical `9c1a2cc352` | identical `9c1a2cc352` | identical `9c1a2cc352` | identical `9c1a2cc352` |
| `vsi_nn_DFPToFp32` | identical `bbbbe165a6` | identical `bbbbe165a6` | identical `bbbbe165a6` | identical `bbbbe165a6` |
| `vsi_nn_DataAsFloat32` | identical `7ef862ce27` | identical `7ef862ce27` | identical `7ef862ce27` | identical `7ef862ce27` |
| `vsi_nn_DescribeStatus` | identical `d1d05633b1` | identical `d1d05633b1` | identical `d1d05633b1` | identical `d1d05633b1` |
| `vsi_nn_DtypeCompare` | identical `df8c7d1a52` | identical `df8c7d1a52` | identical `df8c7d1a52` | identical `df8c7d1a52` |
| `vsi_nn_DtypeConvert` | identical `6176d31ed2` | identical `6176d31ed2` | identical `6176d31ed2` | identical `6176d31ed2` |
| `vsi_nn_DtypeConvertFloat32ToRawData` | identical `5b8aac8e43` | identical `5b8aac8e43` | identical `5b8aac8e43` | identical `5b8aac8e43` |
| `vsi_nn_DtypeConvertRawData` | identical `e5608d87d0` | identical `e5608d87d0` | identical `e5608d87d0` | identical `e5608d87d0` |
| `vsi_nn_DtypeConvertRawDataToFloat32` | identical `dab0cff8b3` | identical `dab0cff8b3` | identical `dab0cff8b3` | identical `dab0cff8b3` |
| `vsi_nn_DtypeToFloat32` | identical `e9cdb143ac` | identical `e9cdb143ac` | identical `e9cdb143ac` | identical `e9cdb143ac` |
| `vsi_nn_DtypeToFp32` | identical `84db05ac66` | identical `84db05ac66` | identical `84db05ac66` | identical `84db05ac66` |
| `vsi_nn_DumpGraphNodeOutputs` | identical `e086b9258b` | identical `e086b9258b` | identical `e086b9258b` | identical `e086b9258b` |
| `vsi_nn_DumpGraphNodeOutputsEx` | identical `136ed4da51` | identical `136ed4da51` | identical `136ed4da51` | identical `136ed4da51` |
| `vsi_nn_DumpGraphToJson` | identical `7c00b70a3b` | identical `7c00b70a3b` | identical `7c00b70a3b` | identical `7c00b70a3b` |
| `vsi_nn_ExecuteGraphLoop` | identical `1480fc7814` | identical `1480fc7814` | identical `1480fc7814` | identical `1480fc7814` |
| `vsi_nn_FasterRCNN_PostProcess` | identical `4de75cc34e` | identical `4de75cc34e` | identical `4de75cc34e` | identical `4de75cc34e` |
| `vsi_nn_Float32ToDtype` | identical `f92b44e70d` | identical `f92b44e70d` | identical `f92b44e70d` | identical `f92b44e70d` |
| `vsi_nn_FlushHandle` | identical `8ef6f36643` | identical `8ef6f36643` | identical `8ef6f36643` | identical `8ef6f36643` |
| `vsi_nn_FormatToString` | identical `5dc46e83e8` | identical `5dc46e83e8` | identical `5dc46e83e8` | identical `5dc46e83e8` |
| `vsi_nn_Fp16ToFp32` | identical `1ce5119e07` | identical `1ce5119e07` | identical `1ce5119e07` | identical `1ce5119e07` |
| `vsi_nn_Fp32ToAffine` | identical `a9832add67` | identical `a9832add67` | identical `a9832add67` | identical `a9832add67` |
| `vsi_nn_Fp32ToBFp16` | identical `1dd0c133a6` | identical `1dd0c133a6` | identical `1dd0c133a6` | identical `1dd0c133a6` |
| `vsi_nn_Fp32ToDFP` | identical `f442224272` | identical `f442224272` | identical `f442224272` | identical `f442224272` |
| `vsi_nn_Fp32ToFp16` | identical `69416b19b5` | identical `69416b19b5` | identical `69416b19b5` | identical `69416b19b5` |
| `vsi_nn_Fp32toDtype` | identical `9694ef146e` | identical `9694ef146e` | identical `9694ef146e` | identical `9694ef146e` |
| `vsi_nn_Free` | identical `d53a4adee5` | identical `d53a4adee5` | identical `d53a4adee5` | identical `d53a4adee5` |
| `vsi_nn_FreeAlignedBuffer` | identical `1c84a2ad1d` | identical `1c84a2ad1d` | identical `1c84a2ad1d` | identical `1c84a2ad1d` |
| `vsi_nn_GenGraphCCode` | identical `7139e4dd29` | identical `7139e4dd29` | identical `7139e4dd29` | identical `7139e4dd29` |
| `vsi_nn_GenerateGraphJson` | identical `d3c844a017` | identical `d3c844a017` | identical `d3c844a017` | identical `d3c844a017` |
| `vsi_nn_GenerateNBG` | identical `0a6d04a105` | identical `0a6d04a105` | identical `0a6d04a105` | identical `0a6d04a105` |
| `vsi_nn_GetElementNum` | identical `1c28e3d045` | identical `1c28e3d045` | identical `1c28e3d045` | identical `1c28e3d045` |
| `vsi_nn_GetGraphVersion` | identical `b66d965cb3` | identical `b66d965cb3` | identical `b66d965cb3` | identical `b66d965cb3` |
| `vsi_nn_GetNode` | identical `28340d331c` | identical `28340d331c` | identical `28340d331c` | identical `28340d331c` |
| `vsi_nn_GetNodesByUids` | identical `40759e1b7e` | identical `40759e1b7e` | identical `40759e1b7e` | identical `40759e1b7e` |
| `vsi_nn_GetOffsetByCoords` | identical `f719574d6a` | identical `f719574d6a` | identical `f719574d6a` | identical `f719574d6a` |
| `vsi_nn_GetPadForOvx` | identical `54c6c8e9cb` | identical `54c6c8e9cb` | identical `54c6c8e9cb` | identical `54c6c8e9cb` |
| `vsi_nn_GetRunTimeVariable` | identical `75443c9f0d` | identical `75443c9f0d` | identical `75443c9f0d` | identical `75443c9f0d` |
| `vsi_nn_GetStrideSize` | identical `a53c73147b` | identical `a53c73147b` | identical `a53c73147b` | identical `a53c73147b` |
| `vsi_nn_GetStrideSizeBySize` | identical `f0e234892b` | identical `f0e234892b` | identical `f0e234892b` | identical `f0e234892b` |
| `vsi_nn_GetTensor` | identical `47714810c3` | identical `47714810c3` | identical `47714810c3` | identical `47714810c3` |
| `vsi_nn_GetTensorHandle` | identical `744b097d09` | identical `744b097d09` | identical `744b097d09` | identical `744b097d09` |
| `vsi_nn_GetTensorIsScalar` | identical `74061534c1` | identical `74061534c1` | identical `74061534c1` | identical `74061534c1` |
| `vsi_nn_GetTensorSize` | identical `ff3f936c7a` | identical `ff3f936c7a` | identical `ff3f936c7a` | identical `ff3f936c7a` |
| `vsi_nn_GetTensors` | identical `080c50f6fa` | identical `080c50f6fa` | identical `080c50f6fa` | identical `080c50f6fa` |
| `vsi_nn_GetTotalBytesBySize` | identical `00fb3cc3df` | identical `00fb3cc3df` | identical `00fb3cc3df` | identical `00fb3cc3df` |
| `vsi_nn_GetTypeBytes` | identical `18427aa358` | identical `18427aa358` | identical `18427aa358` | identical `18427aa358` |
| `vsi_nn_GetVXKernelTypeForShader` | identical `5103ba3de0` | identical `5103ba3de0` | identical `5103ba3de0` | identical `5103ba3de0` |
| `vsi_nn_GetVersion` | identical `e656a7c61e` | identical `e656a7c61e` | identical `e656a7c61e` | identical `e656a7c61e` |
| `vsi_nn_GetVersionMajor` | identical `5103ba3de0` | identical `5103ba3de0` | identical `5103ba3de0` | identical `5103ba3de0` |
| `vsi_nn_GetVersionMinor` | identical `0becf359e1` | identical `0becf359e1` | identical `0becf359e1` | identical `0becf359e1` |
| `vsi_nn_GetVersionPatch` | identical `fe47d7a161` | identical `fe47d7a161` | identical `fe47d7a161` | identical `fe47d7a161` |
| `vsi_nn_HasRNN` | identical `a973344a75` | identical `a973344a75` | identical `a973344a75` | identical `a973344a75` |
| `vsi_nn_InferShape` | identical `6964f9f46a` | identical `6964f9f46a` | identical `6964f9f46a` | identical `6964f9f46a` |
| `vsi_nn_InitTensorsId` | identical `74d0ebe1e8` | identical `74d0ebe1e8` | identical `74d0ebe1e8` | identical `74d0ebe1e8` |
| `vsi_nn_IntegerConvert` | identical `2403d5d817` | identical `2403d5d817` | identical `2403d5d817` | identical `2403d5d817` |
| `vsi_nn_InvalidateHandle` | identical `ca0e717221` | identical `ca0e717221` | identical `ca0e717221` | identical `ca0e717221` |
| `vsi_nn_InvertPermuteShape` | identical `80cacb5f1f` | identical `80cacb5f1f` | identical `80cacb5f1f` | identical `80cacb5f1f` |
| `vsi_nn_InvertShape` | identical `7295dec01c` | identical `7295dec01c` | identical `7295dec01c` | identical `7295dec01c` |
| `vsi_nn_IsBufferAligned` | identical `9733e054ee` | identical `9733e054ee` | identical `9733e054ee` | identical `9733e054ee` |
| `vsi_nn_IsGraphFastMode` | identical `7633216076` | identical `7633216076` | identical `7633216076` | identical `7633216076` |
| `vsi_nn_KernelAddBuildOption` | identical `6f494a96e4` | identical `6f494a96e4` | identical `6f494a96e4` | identical `6f494a96e4` |
| `vsi_nn_KernelAddSource` | identical `87e01b34a4` | identical `87e01b34a4` | identical `87e01b34a4` | identical `87e01b34a4` |
| `vsi_nn_KernelCreate` | identical `72a56f75e6` | identical `72a56f75e6` | identical `72a56f75e6` | identical `72a56f75e6` |
| `vsi_nn_KernelCreateNodeExt` | identical `f6eb00b837` | identical `f6eb00b837` | identical `f6eb00b837` | identical `f6eb00b837` |
| `vsi_nn_KernelDeinitializer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_KernelGpuConfig` | identical `afdbe5b804` | identical `afdbe5b804` | identical `afdbe5b804` | identical `afdbe5b804` |
| `vsi_nn_KernelInitializer` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_KernelNodePassParam` | identical `6293e06411` | identical `6293e06411` | identical `6293e06411` | identical `6293e06411` |
| `vsi_nn_KernelValidator` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_LinkListDeinit` | identical `c1a1e9ea25` | identical `c1a1e9ea25` | identical `c1a1e9ea25` | identical `c1a1e9ea25` |
| `vsi_nn_LinkListDelIndexNode` | identical `301a2e54dc` | identical `301a2e54dc` | identical `301a2e54dc` | identical `301a2e54dc` |
| `vsi_nn_LinkListGetIndexNode` | identical `87a9015c70` | identical `87a9015c70` | identical `87a9015c70` | identical `87a9015c70` |
| `vsi_nn_LinkListGetNodeNumber` | identical `490e1f5156` | identical `490e1f5156` | identical `490e1f5156` | identical `490e1f5156` |
| `vsi_nn_LinkListNewNode` | identical `90515c2691` | identical `90515c2691` | identical `90515c2691` | identical `90515c2691` |
| `vsi_nn_LinkListNext` | identical `3dbdd54b56` | identical `3dbdd54b56` | identical `3dbdd54b56` | identical `3dbdd54b56` |
| `vsi_nn_LinkListPopEnd` | identical `409c24d213` | identical `409c24d213` | identical `409c24d213` | identical `409c24d213` |
| `vsi_nn_LinkListPopStart` | identical `2130807f54` | identical `2130807f54` | identical `2130807f54` | identical `2130807f54` |
| `vsi_nn_LinkListPushEnd` | identical `288ab32cc7` | identical `288ab32cc7` | identical `288ab32cc7` | identical `288ab32cc7` |
| `vsi_nn_LinkListPushStart` | identical `d4d0d1e453` | identical `d4d0d1e453` | identical `d4d0d1e453` | identical `d4d0d1e453` |
| `vsi_nn_LinkListRemoveNode` | identical `f989431dad` | identical `f989431dad` | identical `f989431dad` | identical `f989431dad` |
| `vsi_nn_LoadBinaryData` | identical `cbfbb9a19d` | identical `cbfbb9a19d` | identical `cbfbb9a19d` | identical `cbfbb9a19d` |
| `vsi_nn_LogMsg` | identical `64ca880758` | identical `64ca880758` | identical `64ca880758` | identical `64ca880758` |
| `vsi_nn_Malloc` | identical `bccabf10e5` | identical `bccabf10e5` | identical `bccabf10e5` | identical `bccabf10e5` |
| `vsi_nn_MallocAlignedBuffer` | identical `0ffad4b9cb` | identical `0ffad4b9cb` | identical `0ffad4b9cb` | identical `0ffad4b9cb` |
| `vsi_nn_MapAdd` | identical `5ec0075aa3` | identical `5ec0075aa3` | identical `5ec0075aa3` | identical `5ec0075aa3` |
| `vsi_nn_MapGet` | identical `52b8e1ee8b` | identical `52b8e1ee8b` | identical `52b8e1ee8b` | identical `52b8e1ee8b` |
| `vsi_nn_MapHasKey` | identical `d07cc85db6` | identical `d07cc85db6` | identical `d07cc85db6` | identical `d07cc85db6` |
| `vsi_nn_MapInit` | identical `b881152f6d` | identical `b881152f6d` | identical `b881152f6d` | identical `b881152f6d` |
| `vsi_nn_MapRemove` | identical `7ea799f9fb` | identical `7ea799f9fb` | identical `7ea799f9fb` | identical `7ea799f9fb` |
| `vsi_nn_MapTensorPatch` | identical `1c871209d1` | identical `1c871209d1` | identical `1c871209d1` | identical `1c871209d1` |
| `vsi_nn_Mkdir` | identical `62d6701fe3` | identical `62d6701fe3` | identical `62d6701fe3` | identical `62d6701fe3` |
| `vsi_nn_NewNode` | identical `16911b8702` | identical `16911b8702` | identical `16911b8702` | identical `16911b8702` |
| `vsi_nn_OpCheck` | identical `fbcfc3d39d` | identical `fbcfc3d39d` | identical `fbcfc3d39d` | identical `fbcfc3d39d` |
| `vsi_nn_OpGenerateTensor` | identical `5c0d0021e3` | identical `5c0d0021e3` | identical `5c0d0021e3` | identical `5c0d0021e3` |
| `vsi_nn_OpGetClient` | identical `3969475c6d` | identical `3969475c6d` | identical `3969475c6d` | identical `3969475c6d` |
| `vsi_nn_OpGetName` | identical `c9f25d68f3` | identical `c9f25d68f3` | identical `c9f25d68f3` | identical `c9f25d68f3` |
| `vsi_nn_OpIsRegistered` | identical `6b5dff8b0e` | identical `6b5dff8b0e` | identical `6b5dff8b0e` | identical `6b5dff8b0e` |
| `vsi_nn_OpRegisterClient` | identical `68c491dde6` | identical `68c491dde6` | identical `68c491dde6` | identical `68c491dde6` |
| `vsi_nn_OpRemoveClient` | identical `208ad8a567` | identical `208ad8a567` | identical `208ad8a567` | identical `208ad8a567` |
| `vsi_nn_OptimizeGraph` | identical `739fae1269` | identical `739fae1269` | identical `739fae1269` | identical `739fae1269` |
| `vsi_nn_Pack4bitData` | identical `5f3b2962c8` | identical `5f3b2962c8` | identical `5f3b2962c8` | identical `5f3b2962c8` |
| `vsi_nn_Permute` | identical `7bdaef957e` | identical `7bdaef957e` | identical `7bdaef957e` | identical `7bdaef957e` |
| `vsi_nn_PermuteTensor` | identical `f1129c7316` | identical `f1129c7316` | identical `f1129c7316` | identical `f1129c7316` |
| `vsi_nn_PrintGraph` | identical `4f26de2061` | identical `4f26de2061` | identical `4f26de2061` | identical `4f26de2061` |
| `vsi_nn_PrintNode` | identical `a01f740a53` | identical `a01f740a53` | identical `a01f740a53` | identical `a01f740a53` |
| `vsi_nn_PrintNodeIO` | identical `405e04b896` | identical `405e04b896` | identical `405e04b896` | identical `405e04b896` |
| `vsi_nn_PrintTensor` | identical `d6848b6d3c` | identical `d6848b6d3c` | identical `d6848b6d3c` | identical `d6848b6d3c` |
| `vsi_nn_QuantAffineCalParam` | identical `63da7e9e28` | identical `63da7e9e28` | identical `63da7e9e28` | identical `63da7e9e28` |
| `vsi_nn_QuantAffineCheck` | identical `59b46e4734` | identical `59b46e4734` | identical `59b46e4734` | identical `59b46e4734` |
| `vsi_nn_QuantAffinePerchannelCalParam` | identical `4f359ed26f` | identical `4f359ed26f` | identical `4f359ed26f` | identical `4f359ed26f` |
| `vsi_nn_QuantAffinePerchannelCheck` | identical `ead8919075` | identical `ead8919075` | identical `ead8919075` | identical `ead8919075` |
| `vsi_nn_QuantCheck` | identical `d6a78653bc` | identical `d6a78653bc` | identical `d6a78653bc` | identical `d6a78653bc` |
| `vsi_nn_QuantDFPCalParam` | identical `77bcaad9c0` | identical `77bcaad9c0` | identical `77bcaad9c0` | identical `77bcaad9c0` |
| `vsi_nn_QuantDFPCheck` | identical `9c9e64b869` | identical `9c9e64b869` | identical `9c9e64b869` | identical `9c9e64b869` |
| `vsi_nn_QueryTensorAttr` | identical `8bcb56fd71` | identical `8bcb56fd71` | identical `8bcb56fd71` | identical `8bcb56fd71` |
| `vsi_nn_RegisterClientKernelAndCreateNode` | identical `0270096b5f` | identical `0270096b5f` | identical `0270096b5f` | identical `0270096b5f` |
| `vsi_nn_RegisterClientKernelAndNewNode` | identical `cfc66c5445` | identical `cfc66c5445` | identical `cfc66c5445` | identical `cfc66c5445` |
| `vsi_nn_ReleaseContext` | identical `5a44c342dc` | identical `5a44c342dc` | identical `5a44c342dc` | identical `5a44c342dc` |
| `vsi_nn_ReleaseGraph` | identical `cfb9cfaabc` | identical `cfb9cfaabc` | identical `cfb9cfaabc` | identical `cfb9cfaabc` |
| `vsi_nn_ReleaseGraphJson` | identical `be50aaf7ee` | identical `be50aaf7ee` | identical `be50aaf7ee` | identical `be50aaf7ee` |
| `vsi_nn_ReleaseImageprocessSingleNode` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_ReleaseNode` | identical `0d843f4697` | identical `0d843f4697` | identical `0d843f4697` | identical `0d843f4697` |
| `vsi_nn_ReleaseTensor` | identical `4a22a6c3be` | identical `4a22a6c3be` | identical `4a22a6c3be` | identical `4a22a6c3be` |
| `vsi_nn_ReleaseTensorRelevance` | identical `d53886f14d` | identical `d53886f14d` | identical `d53886f14d` | identical `d53886f14d` |
| `vsi_nn_RemoveNode` | identical `acd782a21b` | identical `acd782a21b` | identical `acd782a21b` | identical `acd782a21b` |
| `vsi_nn_ResetRNNBuffers` | identical `f94154c5a3` | identical `f94154c5a3` | identical `f94154c5a3` | identical `f94154c5a3` |
| `vsi_nn_ReshapeTensor` | identical `812cf0977f` | identical `812cf0977f` | identical `812cf0977f` | identical `812cf0977f` |
| `vsi_nn_Rint` | identical `a50d2028b8` | identical `a50d2028b8` | identical `a50d2028b8` | identical `a50d2028b8` |
| `vsi_nn_RunGraph` | identical `23249ee9f9` | identical `23249ee9f9` | identical `23249ee9f9` | identical `23249ee9f9` |
| `vsi_nn_SaveDataToText` | identical `a6e5eb6b6b` | identical `a6e5eb6b6b` | identical `a6e5eb6b6b` | identical `a6e5eb6b6b` |
| `vsi_nn_SaveTensorToBinary` | identical `e77ed3af6d` | identical `e77ed3af6d` | identical `e77ed3af6d` | identical `e77ed3af6d` |
| `vsi_nn_SaveTensorToText` | identical `ee3baacec7` | identical `ee3baacec7` | identical `ee3baacec7` | identical `ee3baacec7` |
| `vsi_nn_SaveTensorToTextByFp32` | identical `5fe91678e0` | identical `5fe91678e0` | identical `5fe91678e0` | identical `5fe91678e0` |
| `vsi_nn_SetGraphFastMode` | identical `35e635ecc9` | identical `35e635ecc9` | identical `35e635ecc9` | identical `35e635ecc9` |
| `vsi_nn_SetGraphInputs` | identical `9de54b6ffa` | identical `9de54b6ffa` | identical `9de54b6ffa` | identical `9de54b6ffa` |
| `vsi_nn_SetGraphOutputs` | identical `b09dc4df95` | identical `b09dc4df95` | identical `b09dc4df95` | identical `b09dc4df95` |
| `vsi_nn_SetGraphPreloadSize` | identical `e276209f15` | identical `e276209f15` | identical `e276209f15` | identical `e276209f15` |
| `vsi_nn_SetGraphPriority` | identical `58bdeaeb8f` | identical `58bdeaeb8f` | identical `58bdeaeb8f` | identical `58bdeaeb8f` |
| `vsi_nn_SetGraphVersion` | identical `31e7d15122` | identical `31e7d15122` | identical `31e7d15122` | identical `31e7d15122` |
| `vsi_nn_SetNodeBatchSplitNum` | identical `ed4173247f` | identical `ed4173247f` | identical `ed4173247f` | identical `ed4173247f` |
| `vsi_nn_SetRunTimeVariable` | identical `1b0ddbbe05` | identical `1b0ddbbe05` | identical `1b0ddbbe05` | identical `1b0ddbbe05` |
| `vsi_nn_SetTensorAttr` | identical `81ddbbb1f0` | identical `81ddbbb1f0` | identical `81ddbbb1f0` | identical `81ddbbb1f0` |
| `vsi_nn_SetTensorIsScalar` | identical `807f10a24a` | identical `807f10a24a` | identical `807f10a24a` | identical `807f10a24a` |
| `vsi_nn_SetupGraph` | identical `6761c8823c` | identical `6761c8823c` | identical `6761c8823c` | identical `6761c8823c` |
| `vsi_nn_SetupRNNConnections` | identical `0cdbb528fe` | identical `0cdbb528fe` | identical `0cdbb528fe` | identical `0cdbb528fe` |
| `vsi_nn_ShapeProduct` | identical `5ce563e438` | identical `5ce563e438` | identical `5ce563e438` | identical `5ce563e438` |
| `vsi_nn_ShapeToString` | identical `9022b7ab48` | identical `9022b7ab48` | identical `9022b7ab48` | identical `9022b7ab48` |
| `vsi_nn_SqueezeShape` | identical `87da1004c3` | identical `87da1004c3` | identical `87da1004c3` | identical `87da1004c3` |
| `vsi_nn_SwapHandle` | identical `9bfbade4c4` | identical `9bfbade4c4` | identical `9bfbade4c4` | identical `9bfbade4c4` |
| `vsi_nn_SwapInputBuffer` | identical `0541b2e2c7` | identical `0541b2e2c7` | identical `0541b2e2c7` | identical `0541b2e2c7` |
| `vsi_nn_SwapOutputBuffer` | identical `10fb6dc3d5` | identical `10fb6dc3d5` | identical `10fb6dc3d5` | identical `10fb6dc3d5` |
| `vsi_nn_SwapTensorHandle` | identical `4e08eaf397` | identical `4e08eaf397` | identical `4e08eaf397` | identical `4e08eaf397` |
| `vsi_nn_SwapTensorHandleWithCache` | identical `da2554ad9a` | identical `da2554ad9a` | identical `da2554ad9a` | identical `da2554ad9a` |
| `vsi_nn_SwitchInputBuffer` | identical `c1f1593d54` | identical `c1f1593d54` | identical `c1f1593d54` | identical `c1f1593d54` |
| `vsi_nn_SwitchOutputBuffer` | identical `0b95938d76` | identical `0b95938d76` | identical `0b95938d76` | identical `0b95938d76` |
| `vsi_nn_Transpose` | identical `ef2d0b8552` | identical `ef2d0b8552` | identical `ef2d0b8552` | identical `ef2d0b8552` |
| `vsi_nn_TransposeTensor` | identical `1fc7bb2aa8` | identical `1fc7bb2aa8` | identical `1fc7bb2aa8` | identical `1fc7bb2aa8` |
| `vsi_nn_TrySetupCompleteSignalNode` | identical `9c1cf8c7c4` | identical `9c1cf8c7c4` | identical `9c1cf8c7c4` | identical `9c1cf8c7c4` |
| `vsi_nn_TypeGetBits` | identical `5f571e8c37` | identical `5f571e8c37` | identical `5f571e8c37` | identical `5f571e8c37` |
| `vsi_nn_TypeGetBytes` | identical `556c51e102` | identical `556c51e102` | identical `556c51e102` | identical `556c51e102` |
| `vsi_nn_TypeGetBytesExt` | identical `16fdad5a5c` | identical `16fdad5a5c` | identical `16fdad5a5c` | identical `16fdad5a5c` |
| `vsi_nn_TypeIsInteger` | identical `d13e47205f` | identical `d13e47205f` | identical `d13e47205f` | identical `d13e47205f` |
| `vsi_nn_TypeIsSigned` | identical `e0db0e04f4` | identical `e0db0e04f4` | identical `e0db0e04f4` | identical `e0db0e04f4` |
| `vsi_nn_UnmapTensorPatch` | identical `db8a7feed6` | identical `db8a7feed6` | identical `db8a7feed6` | identical `db8a7feed6` |
| `vsi_nn_Unpack4bitData` | identical `85a040f03a` | identical `85a040f03a` | identical `85a040f03a` | identical `85a040f03a` |
| `vsi_nn_UpdateCropParamsForBinaryGraph` | identical `7594e71c14` | identical `7594e71c14` | identical `7594e71c14` | identical `7594e71c14` |
| `vsi_nn_UpdateTensorDims` | identical `850a3e93a6` | identical `850a3e93a6` | identical `850a3e93a6` | identical `850a3e93a6` |
| `vsi_nn_VariableToTensor` | identical `59b190edc9` | identical `59b190edc9` | identical `59b190edc9` | identical `59b190edc9` |
| `vsi_nn_VerifyGraph` | identical `9e32c41119` | identical `9e32c41119` | identical `9e32c41119` | identical `9e32c41119` |
| `vsi_nn_VxBinResourceGetResource` | identical `d3c844a017` | identical `d3c844a017` | identical `d3c844a017` | identical `d3c844a017` |
| `vsi_nn_VxResourceGetPath` | identical `fb1dd0eabb` | identical `fb1dd0eabb` | identical `fb1dd0eabb` | identical `fb1dd0eabb` |
| `vsi_nn_VxResourceSetPath` | identical `cc6d11f77d` | identical `cc6d11f77d` | identical `cc6d11f77d` | identical `cc6d11f77d` |
| `vsi_nn_compareVersion` | identical `424189a827` | identical `424189a827` | identical `424189a827` | identical `424189a827` |
| `vsi_nn_initOptions` | identical `4fb644cfeb` | identical `4fb644cfeb` | identical `4fb644cfeb` | identical `4fb644cfeb` |
| `vsi_nn_initOptions_runtime` | identical `449c99cfd0` | identical `449c99cfd0` | identical `449c99cfd0` | identical `449c99cfd0` |
| `vsi_nn_is_do_vx_op_pre_init` | identical `2c600bb53e` | identical `2c600bb53e` | identical `2c600bb53e` | identical `2c600bb53e` |
| `vsi_nn_kernelScalarCreate` | identical `060eb21e16` | identical `060eb21e16` | identical `060eb21e16` | identical `060eb21e16` |
| `vsi_nn_op_common_compute` | identical `be50aaf7ee` | identical `be50aaf7ee` | identical `be50aaf7ee` | identical `be50aaf7ee` |
| `vsi_nn_op_common_deinit` | identical `8ad35e1c98` | identical `8ad35e1c98` | identical `8ad35e1c98` | identical `8ad35e1c98` |
| `vsi_nn_op_common_optimize` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_op_common_setup` | identical `b85b20e8de` | identical `b85b20e8de` | identical `b85b20e8de` | identical `b85b20e8de` |
| `vsi_nn_op_eltwise_setup` | identical `26d4ed53b8` | identical `26d4ed53b8` | identical `26d4ed53b8` | identical `26d4ed53b8` |
| `vsi_nn_op_imageprocess_single_node` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` | identical `2f6110664d` |
| `vsi_nn_rnn_ResetBuffers` | identical `6147bb1366` | identical `6147bb1366` | identical `6147bb1366` | identical `6147bb1366` |
| `vsi_nn_rnn_RunGraph` | identical `32f95531f4` | identical `32f95531f4` | identical `32f95531f4` | identical `32f95531f4` |
| `vsi_nn_vxConvertFloat32DataToTensor` | identical `760b07f405` | identical `760b07f405` | identical `760b07f405` | identical `760b07f405` |
| `vsi_nn_vxConvertTensorToFloat32Data` | identical `e971304979` | identical `e971304979` | identical `e971304979` | identical `e971304979` |
| `vsi_nn_vxCopyDataToTensor` | identical `1bd20fd4e7` | identical `1bd20fd4e7` | identical `1bd20fd4e7` | identical `1bd20fd4e7` |
| `vsi_nn_vxCopyTensorToData` | identical `f063210801` | identical `f063210801` | identical `f063210801` | identical `f063210801` |
| `vsi_nn_vxGetTensorAttr` | identical `e052c1b5bb` | identical `e052c1b5bb` | identical `e052c1b5bb` | identical `e052c1b5bb` |
| `vsi_nn_vxGetTensorElementNum` | identical `28a0c94fc3` | identical `28a0c94fc3` | identical `28a0c94fc3` | identical `28a0c94fc3` |
