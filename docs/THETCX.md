# OpenRevo · 极光 X Pro 专用分支 (`thetcx`)

只给一台机器用：MECHREVO 极光 X Pro（主板 `KUANGSHI-JIGUANG Series-X6xx57TW`，EC ProjectID 26），
i7-14650HX + RTX 5070 Ti Laptop（65 W 底 + 50 W cTGP + 25 W DB）。

## 和上游 0.9.3 的区别

**安全**
- 主板名不是 `KUANGSHI-JIGUANG Series-X6xx57TW` 时，所有 EC 写入直接拒绝（`machine.rs`）。
- 删除：插件市场 / Rhai 脚本 / Widget / 原始 EC 读写网关（插件可任意写 EC）、上游自动更新（会把本分支覆盖成原版）、
  BLD 端口 I/O 驱动与其内核驱动文件、外部驱动清单加载、“高级 BIOS 超频菜单”（写 UEFI NVRAM）。
- 修复 `wmi_write_ec_batch` 回退路径的死锁（重复加 ACPI 锁）。

**功耗 / 风扇**
- 预设模式 GPU 按官方：办公 65 W、均衡 115 W、狂暴 115+25 = 140 W（上游给的是 115 W / 140 W / 140 W）。
- GPU 功率包络写死 65/115/25/140，不再依赖 nvidia-smi（独显休眠时查询为空，上游会退回 50/100/15 W 并整场生效）。
- 预设模式风扇表改为分模式调校（「风扇调度」页可切回官方原厂表）：

  | 模式 | 起转 | 70 °C | 满速 |
  |---|---|---|---|
  | 办公 | 55 °C | 42 % | 90 °C |
  | 均衡 | 50 °C | 55 % | 90 °C |
  | 狂暴 | 45 °C | 75 % | 85 °C |
  | 官方原厂（三模式相同） | 48 °C | 70 % | 85 °C |

- 自定义曲线按官方 UpT/DownT 回差语义生成（上游回差偏大，降速慢）。
- 每次写风扇表后读回校验并记日志。
- TjMax 固定 100 °C（温度墙按 TCC Offset 写入）。

**体验**
- 全局快捷键默认关闭，可在「系统」页开启（开启后拦截按键，不再与 VS Code 冲突）。
- 开启接管官方服务时自动打开开机自启；接管中但没开自启会在总览页和系统页报警。
- 空闲开销：机型配置/灯效契约缓存（上游每 500 ms 解析 340 KB JSON）、`schtasks` 查询缓存（上游每 1.5 s 启一次进程）、
  键盘钩子不再转发每个按键。

## 早上实机测试清单

替换前先退出托盘里的 OpenRevo，然后：

```powershell
Copy-Item E:\OpenRevo-v0.9.3-portable\open-revo.exe E:\OpenRevo-v0.9.3-portable\open-revo.v0.9.3-upstream.exe
Copy-Item E:\ClaudeformygamePC\CCX_RE\builds\open-revo-thetcx.exe E:\OpenRevo-v0.9.3-portable\open-revo.exe
```

开机自启的计划任务指向这个路径，不用改。启动后：

1. 「系统」→ 临时打开日志（方便核对），总览页确认不是 SAFE MOCK、读数正常。
2. 切 **办公** → 「风扇调度」→「读取」：CPU 温度行应为 `0 55 60 65 70 75 80 85 90 …`，转速行 `0 25 30 35 42 50 62 80 100`。
   烤机时 GPU 功耗应封顶约 65 W。
   **关键判据**：空闲 CPU 在 50~54 °C 时风扇应停转（调校表 55 °C 才起转，官方表 48 °C 就转）。
   读回的表对但风扇照转，说明预设模式下 EC 不用 RAM 表——这种情况告诉我，我改成只在自定义模式写表。
3. 切 **均衡**：读取应为 `0 50 55 …`；游戏时 GPU 上限 115 W。
4. 切 **狂暴**：读取 `0 45 50 …`；GPU 上限 140 W，CPU 175 W。
5. 切回 **自定义**：确认 PL1/PL2/PL4 125/175/230、GPU 140 W、超频 +100/+255 生效；风扇按自定义曲线。
6. 「风扇调度」切到 **官方原厂** 再读取：应为 `0 48 52 56 … 85`、`0 30 35 40 50 55 60 70 80 90 100`。
7. 日志里搜 `Fan table readback`：应为 `verified`；若出现 `mismatch`，说明 EC 没接受写入，告诉我。

任何一步风扇狂转/不转、黑屏、降频到 0.8 GHz：立刻点「一键强冷」或切官方原厂表，再按下面回滚。

## 回滚

- 换回上游：退出 OpenRevo，把 `open-revo.v0.9.3-upstream.exe` 改回 `open-revo.exe`。
- 回官方控制中心：在 OpenRevo 里关闭「接管官方控制台」（会恢复 GCUBridge 自动启动），或手动
  `sc config GCUBridge start= auto` + `sc start GCUBridge`（管理员）。

## 构建

必须用 `pnpm tauri build --no-bundle`（产物 `src-tauri/target/release/open-revo.exe`）。
直接 `cargo build --release` 不会开启 `custom-protocol`，界面会去连开发服务器 `localhost:1420`，打开就是 ERR_CONNECTION_REFUSED。
