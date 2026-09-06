one_bridge_canister 修复说明（2026-09-06）

对应审查基准 a2498a838869ef513dbc6bad3e6f90f53428a951。保留原有接口，新增字段通过可选的 `runtime` 扩展传递，以兼容尚未升级的桥。原审查中的源码行号对应基准版本；当前实现已按职责拆分。

F03 按用户明确选择处理：继续使用 `is_replicated=false`，控制者配置独立的官方 provider，资金核心读需两个 provider 的一致或保守结果。相同 host、端口/path 别名和已识别的同一供应商域名不增加票数。该方案明确依赖执行 outcall 的 IC 节点，不宣称能够抵抗恶意单节点伪造 HTTPS 响应。

| 清单 | 实施结果 |
| --- | --- |
| F01 | 稳定保存每小时全局/用户额度，限制活跃请求、pending 和未决入金；基础 cycles 储备之外，按当前及已挂起的补贴请求各预留 100B cycles；在请求保护函数中先计额度再做 RPC/签名。治理可调整限额与基础储备。 |
| F02 | 入金、出金、提现、手续费注资均记录稳定操作；固定 ledger、时间、memo 和参数；ledger 等待限制为 60 秒，超时按未知结果继续使用原请求；未知结果不释放预占；已完成操作覆盖迟到回调；签名调用前记录承诺。 |
| F03 | 保持非复制；官方 provider 由治理批准，独立身份检查、双源核心证据及文档信任边界已落实。 |
| F04 | Solana 使用已被两源识别的实际 blockhash 与 context slot；过期需要 finalized 视图、实际 hash 失效、历史覆盖和签名缺失；不使用单方广告 deadline 推断过期。 |
| F05 | gas/tip 使用两源较高报价，避免单个过低报价固化不可打包交易；随后应用每链单价上限、单笔总额上限和持久化每小时出金 gas 预算。 |
| F06 | 只允许经典 SPL 及无 mint 扩展的 Token-2022 用于桥接；转账费/hook 等语义在启用前拒绝。租金按账户尺寸双源读取；保留向已有 ATA 取回旧扩展 mint 的用户资金路径。 |
| F07 | EVM 出金检查 token、桥地址、收款地址与净额 Transfer 证据；不足或缺失时保留债务，不自动全额重付。 |
| F08 | receipt 保留 blockHash 并检查 canonical block；replacement 使用符合 finality 的 nonce；Solana 错误只有 finalized 后才是终局失败；旧的缺失历史进入核对状态。 |
| F09 | 稳定 due-time 索引公平调度；EVM nonce 保留独立于全局优先级；长期在途的三个 EVM 任务不会挤占其他链。 |
| F10 | 首次 await 前设置独立看门狗；逐任务提交结果；trap 清理释放锁并调整租约；迟到轮次不能覆盖新轮次。 |
| F11 | HTTP、解析及 JSON-RPC 服务错误均可故障切换；恰好所需的两个读并发；错误长度受限并脱敏。 |
| F12 | 扣款前检查源/目标金额转换、非零、乘法溢出、Solana u64 上限和精度；改目标时复用检查。 |
| F13 | 注册回调原子复检唯一性、容量和 provider 快照；ledger 身份固定在操作和任务中；有财务历史/意图时禁止直接替换 ledger。 |
| F14 | 内部 provider 与明确公开的浏览器 RPC 分开；默认只公开无 path/query 的原始匿名端点，提供不含凭据的 host 概览，不制造无效的脱敏 URL。 |
| F15 | pending 存于 StableBTreeMap，用户/交易/due/nonce/故障链共用命名空间索引；查询分页；迁移和重启唤醒按批执行；来源交易保留去重墓碑。 |
| F16 | 旧 pending 的 fee 默认为零；按权威归档和持久游标恢复索引，覆盖回滚期间的新增日志；同一来源交易的重复旧记录完整写入核账 journal，不在迁移中丢弃；旧布局通过 Wasm 升级 fixture 验证。 |
| F17 | ATA 检查只解释存在性，余额变化不造成假分歧。 |
| F18 | 错误链作为结构化字段和索引保存；ETHW 不会误挡 ETH；文字前缀仅用于旧格式迁移。 |
| O01 | Solana 批量状态读取，三个签名双源查询由六次请求减为两次；保留 EVM 轮次缓存；双源 RPC 并发且无多余常规请求。 |
| O02 | Solana 消息加入稳定操作 ID 的 Memo，区分不同业务请求；同一签名意图复用已记录交易。 |
| O03 | 分离 model、pending、journal、budget、migration、state、engine、transactions 与测试；出金终局状态和操作阶段显式表示；删除已不被生产路径使用的旧队列实现及测试。 |
| O04 | 普通 retry 不清除未知付款；recheck 只重新查证；治理核账要求精确 revision、链类型、已知交易一致性和留存证据；核账结论改变时反向修正原账务；带 journal 的 payout 未终结前不能强制关闭任务，孤儿 Completed payout 会自动关闭 journal。 |
| O05 | ledger decimals/minting account 初始化核对，拒绝匿名治理、非法 ledger 和元数据；实际新转账费用与预占单独记账；历史收入经核账后才恢复可支配额度。 |
| O06 | `/config` 和 `/config.cbor` 缓存并认证配置/资金地址；HEAD 与 GET 的证据分开；修复 Accept q=0；保留旧 HTTP 统计接口的既有信任语义。 |
| O07 | PocketIC 中验证未知回复、回调 trap、升级重试、看门狗及迟到回调；旧格式迁移、RPC/状态机回归、Candid 漂移检查、严格 lint；固定 Rust、构建镜像 digest 和 CI actions；gzip 不写时间戳。 |

默认资源政策：普通用户每小时 12 次昂贵请求，全局每小时 120 次，最多 16 个活跃请求；pending/未决入金容量默认 512，单用户 pending 默认 32；保留 2T cycles，并为当前请求及每个已挂起的补贴请求再预留 100B cycles。失败请求也消耗额度，避免未资助地址重复发起免费 RPC。控制者/治理可进行恢复，并可通过 `admin_set_resource_limits` 调整补贴政策的额度与基础储备。周期按 IC 时间整点小时计算。

默认 EVM 费用上限为 500 gwei max fee、25 gwei tip、每笔 0.05 native coin、每链每小时 0.5 native coin（按签名交易的最坏支出保守预留）。治理应根据链的实际费用设置 `admin_set_evm_fee_limits`；该预算与用户派生地址的余额检查分别存在。

升级与运营步骤：

1. 常规升级保持原 canister ID、ledger、密钥名称及 MemoryId。升级后检查 `info().runtime` 中的 `ledger_verified`、`keys_ready`、`svm_mint_verified`、`migration_remaining`，待验证/迁移完成再接受新请求。初始化失败可调用 `admin_init_public_keys` 并查看具体错误。
2. 私有或带 path/query 的内部 RPC 若不能直接公开，应以 `admin_set_public_providers` 配置匿名官方浏览器端点。公开端点是明确的公开配置，不应填入 API key。原本无 path/query 的匿名 provider 可继续供旧前端使用。
3. 历史 `icp_collected_fees` 是收入统计，旧版本没有完整支出日志。升级不自动把历史毛收入当作可提现资金。可先由项目方在 ledger 授权后调用 `fund_ledger_fees` 注入运行费；完成外部资产/负债核对后，治理调用 `admin_recognize_legacy_fees` 设置已核实的历史费用累计总额。重复相同累计总额不会重复增加额度。
4. 新客户端宜调用 `bridge_with_id`，对同一操作保留同一 request ID。旧 `bridge` 在存在同参未决入金时仍可恢复；若随后改用 `bridge_with_id` 接管，该 ID 会持久绑定到原操作。通过 `my_operations` 查看提交/未知/完成阶段，使用 `resume_operation` 或 `resume_deposit` 恢复；只有已知未执行的操作才能取消。
5. `recheck_task`/`admin_recheck_task` 保留原签名和去重键重新查证；普通 `admin_retry_bridging_task` 仅允许安全重建。`admin_resolve_operation` 与 `admin_resolve_legacy_payout` 是治理完成外部核账后的明确事实认定，不是自动区块链不存在性证明。它们保存证据并验证操作版本，不能用“暂时查不到”作为未执行依据。存在 `payout_attempt` 时先终结其 operation，再关闭任务。
6. 如需回滚到不了解新 pending/journal 布局的旧代码，先清空并核对全部未决资金意图。新稳定表的 MemoryId 不可复用；再次升级会补齐回滚期间的旧格式归档。不要在存在新格式在途资金时直接运行旧版本。
7. 新增管理方法各有 `validate_*` 对应方法。SNS 部署须另行注册相应 generic functions；本次只修改代码与文档，没有创建提案、部署或调用生产资金接口。

验证入口：

```text
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
make integration-test
make build-wasm
make build-did
cargo audit
```

PocketIC 使用独立的测试 Wasm 和本地 mock ledger/RPC。故障注入仅存在于 `test-hooks` 构建，测试检查标准生产 Wasm 不含该入口。测试专用编译输出与默认部署目录分开。

依赖核查已用支持 CVSS 4.0 的 cargo-audit 0.22.2 重新执行，并更新 bytes、ruint、time、anyhow、keccak、rand 等受影响兼容版本。停止维护提示与可利用漏洞分开记录；bincode 1.3 的 Solana wire 编码保留且有固定向量测试，其他相关上游依赖仍需跟进替代版本，未把停止维护提示伪装成漏洞已消失。

最终验证结果：

- `cargo test --locked --workspace`：84 个单元/协议测试通过。集成测试在普通测试运行中显式忽略，通过专用入口独立执行。
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` 与格式检查通过。
- PocketIC 15.0.0：入口回调 trap、定时器回调 trap、未知 ledger 回复、升级后相同请求重试、无 ID 未决入金由新 request ID 接管且只扣款一次、看门狗接管和迟到回复、健康链进展、实际请求额度拒绝均通过。
- PocketIC 中验证了实际管理 canister 的 Ed25519 签名；两笔相同金额/收款人/近期 blockhash 的 Solana 付款含不同操作 Memo，不产生相同消息。
- HTTP 配置的 IC 证书、Merkle witness 和完整配置响应树验证通过。
- 旧版无 fee pending 的升级及回滚期间新增归档再迁移通过。
- `scripts/test-canister.sh` 在独立的绝对 `CARGO_TARGET_DIR` 下从零构建并通过；fixture、生产 Wasm 与故障注入 Wasm 均来自该目录，两次生产构建的 SHA-256 一致。
- release Wasm 构建与 Candid 提取一致；新服务兼容旧接口，并对原有方法子集通过双向 Candid 类型检查。
- 前端 `npm run check`：0 错误、0 警告；`npm run build` 通过，声明文件已同步。
- cargo-audit 0.22.2：主 Cargo.lock 的已知漏洞和 unsound 提示清零；保留 6 项上游停止维护提示并记录，未忽略漏洞。

当前未优化的 release Wasm 为 3,378,154 字节，gzip（mtime=0）为 1,052,181 字节；新日志、恢复接口和认证功能增加了代码体积。性能收益主要来自减少 RPC 往返、批量状态查询、稳定索引和有界迁移，不宣称所有指标都下降。生产发布流程仍会进行 ic-wasm/wasm-opt 优化。

补丁复核独立检查了公开入口、管理验证方法、跨 await 状态、未知结果与迟到回调、来源去重、升级/回滚和前端接口兼容。未运行生产链交易，也未部署或创建治理提案。治理手动核账的真实性仍由控制者负责；F03 的 IC 节点信任假设按用户选择保留。
