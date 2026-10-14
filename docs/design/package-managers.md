# 包管理器分发（winget / Scoop）

目标：`winget install windinput`、`scoop install windinput-np`（目前在自有桶：先
`scoop bucket add huanfeng https://github.com/huanfeng/scoop-bucket`）。两者都只是**调用标准安装包**
`WindInput-Setup-<版本>.exe`，不另出产物；输入法要注册 TSF、写 HKLM，没有真正的便携形态。

| | winget | Scoop |
|---|---|---|
| 仓库 | microsoft/winget-pkgs，`WindInput.WindInput`（Moniker `windinput`） | 自有桶 huanfeng/scoop-bucket，`windinput-np`；官方 Nonportable 待收录（[#647](https://github.com/ScoopInstaller/Nonportable/issues/647)） |
| 清单源 | `scripts/pkg-managers/winget-manifest.sh`（仅首次 / 补交） | `scripts/pkg-managers/scoop/windinput-np.json` |
| 跟进新版 | `release-published.yml` 的 `winget` job（komac，以库里上一版为模板） | `release-published.yml` 的 `scoop` job 发 dispatch，桶内 `update-installer-manifest.yml` 按清单的 `autoupdate` 模板改 version/url/hash（桶的 Excavator 已停用）；Nonportable 由其机器人按 `checkver` 跟进 |
| 下载地址 | `dl.windinput.com`（旧版由网关回落 GitHub） | 同左 |

## 与安装器的契约（wind-installer）

清单里的每个开关都对应 wind-installer 的一条既有行为，改那边要回来对这里：

- **提权**：`Setup.exe` 与 `uninstall.exe` 都内嵌 `requireAdministrator`，系统在进程启动前就弹
  UAC，调用方拿到的是提权后那个进程的句柄，能正常等待与取退出码 ⇒ winget 写
  `ElevationRequirement: elevatesSelf`，Scoop 用 `Invoke-ExternalCommand -RunAs`。
  ⚠️ Scoop 里**不能用 `Start-Process -Wait`**：PowerShell 5 的 `-Wait` 等的是整棵进程树，而安装器
  装完会拉起常驻的 `wind_input.exe`，于是永远等不到、`scoop install` 卡在「Running installer
  script...」。`Invoke-ExternalCommand` 只 `WaitForExit()` 安装器本身，并用 `-ContinueExitCodes`
  把 3010 当成功。
  （安装器里另有一段「未提权就 `runas` 重启自身并 `exit(0)`」的兜底，有清单在就走不到。）
- **开关**：`--silent` 完全无界面；`--quiet` 显示进度、成功且无需重启时自动关。winget 的
  `Silent` 与 `SilentWithProgress`（默认模式）**都映射到 `--silent`**：`--quiet` 在失败或需要
  重启时停在完成页等人关，而原地升级时旧 DLL 被占用、几乎必然需要重启，winget 就卡住不返回。
- **退出码**：`3010` = 成功但需重启，`1618` = 另一实例在运行（没做任何事）。
- **ARP**：键名 = `config/app.toml` 的 `[app] display_name`（「清风输入法」），即 winget 的
  `ProductCode`，也是 Scoop 卸载脚本找安装目录的入口。**改 display_name 会让存量安装与两边
  清单同时失联**。
- **卸载保留数据**：`QuietUninstallString` 带 `--keep-user-data`（wind-installer `fix(installer)`
  那次加的）；Scoop 卸载脚本自己也传。包管理器的惯例是卸程序、留数据，要删数据走控制面板向导。
  ⚠️ 此改动之前打出的安装包，ARP 里只有 `--silent`，`winget uninstall` 会连用户词库一起删——
  所以 **winget 首次入库要用包含该修复的版本**。

## Scoop 清单的两个要点

- `scoop update` 会先跑旧版的 `uninstaller` 再跑新版的 `installer`。安装器本身就支持原地升级，
  先卸再装只会白白反注册一次输入法，故卸载脚本在 `$cmd -eq 'update'` 时直接 `return`
  （`$cmd` 是 Scoop `exec($cmd, …)` 的参数，脚本块按动态作用域读得到）。
- 安装目录从 ARP 的 `InstallLocation` 读，不假定 `Program Files\WindInput`：用户可能在向导里改过。

## 应用内升级与包管理器并存

设置程序的在线升级会直接把程序升到新版，包管理器记录的版本随之落后。winget 按 ARP 的
`DisplayVersion` 判断已装版本，不受影响；Scoop 会以为还是旧版，下次 `scoop update` 无非再
原地装一遍同版或更新版，无害。故不为包管理器安装关闭应用内升级。

## 首次上架步骤

1. 等一个**包含 `--keep-user-data` 修复**的正式版发布——即 0.126.0（此前各版未正式发布，不入库）。
2. winget：`scripts/pkg-managers/winget-manifest.sh <版本> <输出目录>` → 在 Windows 上
   `winget validate --manifest <目录>`、`winget install --manifest <目录>` 实测 → 向
   microsoft/winget-pkgs 提 PR（也可用 `wingetcreate submit`）。
3. 配置 `secrets.WINGET_TOKEN`（classic PAT，`public_repo`），并在该账号下 fork winget-pkgs；
   此后每次 Release 发布由 CI 自动提 PR。
4. Scoop：
   - 自有桶：`windinput-np.json` 原样放进 huanfeng/scoop-bucket 的 `bucket/`；WindInput 的
     `secrets.SCOOP_TOKEN`（对该桶 contents:write）配好后，每次 Release 由 `scoop` job 自动升版。
   - 官方 Nonportable：桶规定先开 Package Request issue、经维护者认可再提 PR
     （`bucket/windinput-np.json`），之后由桶机器人跟进。
   - 本仓那份只作为源稿，**不会**自动同步到任何桶——改安装/卸载脚本要另去两个桶各改一次。
