**PANDA 桥升级：505**

[505](./proposal-505.sh) 参考 493 提案，将桥 canister 从 v0.5.2 升级至 v0.6.1。
脚本使用 GitHub Release 发布的 WASM，校验 SHA-256 后通过 `quill` 生成
`proposal-message.json`，不自动提交；不传升级参数，保留现有费用、最低桥接金额等配置。
运行前按脚本注释下载发布产物，并复核链上模块哈希。
原 505 的治理函数注册提案尚未执行，现改为 [512](./proposal-512.sh)，内容不变。

**PANDA 桥治理函数注册：506–512**

这批脚本使用与既有注册提案相同的 SNS 治理 canister、neuron 子账户和
`ApplicationBusinessLogic` topic，添加以下治理入口。注册提案通过后，实际调用目标方法仍需单独的执行提案。

| 脚本编号                 | SNS function ID | 目标方法                      | 验证方法                               |
| ------------------------ | --------------- | ----------------------------- | -------------------------------------- |
| [512](./proposal-512.sh) | 1311            | `admin_init_public_keys`      | `validate_admin_init_public_keys`      |
| [506](./proposal-506.sh) | 1312            | `admin_set_resource_limits`   | `validate_admin_set_resource_limits`   |
| [507](./proposal-507.sh) | 1313            | `admin_set_evm_fee_limits`    | `validate_admin_set_evm_fee_limits`    |
| [508](./proposal-508.sh) | 1314            | `admin_resolve_operation`     | `validate_admin_resolve_operation`     |
| [509](./proposal-509.sh) | 1315            | `admin_resolve_legacy_payout` | `validate_admin_resolve_legacy_payout` |
| [510](./proposal-510.sh) | 1317            | `admin_recheck_task`          | `validate_admin_recheck_task`          |
| [511](./proposal-511.sh) | 1318            | `admin_set_public_providers`  | `validate_admin_set_public_providers`  |

目标和 validator canister 均为 `dpjyw-raaaa-aaaar-qbxlq-cai`；
治理 canister 为 `dwv6s-6aaaa-aaaaq-aacta-cai`。
这些编号是本地脚本编号，实际链上提案 ID 由 SNS 分配。
原计划的 510 / function ID 1316 已随未上线的费用认领功能取消，其他编号保持不变。
2026-09-10 的只读查询确认 function ID 1311–1318 不在已注册列表或 reserved IDs 中；
提交时应以当时的链上状态为准。

在仓库根目录使用具备该 neuron 提案权限的 dfx 身份运行，例如：

```bash
bash proposals/proposal-512.sh
```

运行脚本会直接调用治理 canister 的 `manage_neuron` 创建提案，方式与既有注册脚本一致。
函数编号对照同时维护在 [sns_functions.md](../sns_functions.md)，新条目标记为待治理注册。

注册可以先于桥代码升级准备；执行新方法前，目标 canister 必须已升级到包含对应方法与 validator 的版本。
资源限额和 gas 限额的具体参数由后续执行提案给出。
PANDA ledger 转账手续费直接由桥现有账户支付，不需要额外注资或历史费用认领提案。
