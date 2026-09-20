**PANDA 桥治理函数注册：505–511**

这批脚本使用与既有注册提案相同的 SNS 治理 canister、neuron 子账户和
`ApplicationBusinessLogic` topic，添加以下治理入口。注册提案通过后，实际调用目标方法仍需单独的执行提案。

| 脚本编号                 | SNS function ID | 目标方法                      | 验证方法                               |
| ------------------------ | --------------- | ----------------------------- | -------------------------------------- |
| [505](./proposal-505.sh) | 1311            | `admin_init_public_keys`      | `validate_admin_init_public_keys`      |
| [506](./proposal-506.sh) | 1312            | `admin_set_resource_limits`   | `validate_admin_set_resource_limits`   |
| [507](./proposal-507.sh) | 1313            | `admin_set_evm_fee_limits`    | `validate_admin_set_evm_fee_limits`    |
| [508](./proposal-508.sh) | 1314            | `admin_resolve_operation`     | `validate_admin_resolve_operation`     |
| [509](./proposal-509.sh) | 1315            | `admin_resolve_legacy_payout` | `validate_admin_resolve_legacy_payout` |
| [510](./proposal-510.sh) | 1316            | `admin_recheck_task`          | `validate_admin_recheck_task`          |
| [511](./proposal-511.sh) | 1317            | `admin_set_public_providers`  | `validate_admin_set_public_providers`  |

目标和 validator canister 均为 `dpjyw-raaaa-aaaar-qbxlq-cai`；
治理 canister 为 `dwv6s-6aaaa-aaaaq-aacta-cai`。
这些编号是本地脚本编号，实际链上提案 ID 由 SNS 分配。
原计划的 510 / function ID 1316 已随未上线的费用认领功能取消，其他编号保持不变。
2026-09-10 的只读查询确认 function ID 1311–1318 不在已注册列表或 reserved IDs 中；
提交时应以当时的链上状态为准。

在仓库根目录使用具备该 neuron 提案权限的 dfx 身份运行，例如：

```bash
bash proposals/proposal-505.sh
```

运行脚本会直接调用治理 canister 的 `manage_neuron` 创建提案，方式与既有注册脚本一致。
函数编号对照同时维护在 [sns_functions.md](../sns_functions.md)，新条目标记为待治理注册。

注册可以先于桥代码升级准备；执行新方法前，目标 canister 必须已升级到包含对应方法与 validator 的版本。
资源限额和 gas 限额的具体参数由后续执行提案给出。
PANDA ledger 转账手续费直接由桥现有账户支付，不需要额外注资或历史费用认领提案。
