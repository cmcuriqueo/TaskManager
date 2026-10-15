# Taskmgr.exe — análisis y puerto a Linux

Administrador de tareas de Windows 11, analizado en estático para reimplementarlo en Rust sobre Linux. El binario es código nativo de Microsoft. Este documento recoge la estructura observada y el diseño del puerto. El código nuevo es una implementación propia a partir de esa estructura.

## Binario

| Campo | Valor |
|---|---|
| Archivo | `Taskmgr.exe` |
| SHA-256 | `affb1e96bb51c4c96a4298ab100cd37f44e6b2b8a56d17bb4de41547727d5827` |
| Formato | PE32+, x86_64 (`0x8664`), imagen `0x140000000`, subsistema GUI |
| Tamaño | 5 716 104 bytes, 9 secciones |
| Versión | 10.0.26100.8972 (WinBuild.160101.0800) |
| Descripción | Task Manager |
| Nombre original | `Taskmgr.exe` |
| Compañía | Microsoft Corporation |
| Ensamblado | `Microsoft.Windows.Diagnosis.AdvancedTaskManager` 5.1.0.0, `amd64` |
| Privilegios | `asInvoker` con `autoElevate`, DPI aware |
| PDB | `Taskmgr.pdb` |

Evidencia REA:

- Recursos PE: `ev_6c09f1bcbf57fe33c0a8412534d8f9935bc080906dccb1394b021222f6321199`
- Metadatos .NET: `ev_5bd8965f03bbad1d6d87583943d550a095eac06ef0b1eb4f1d4407ef6c50d0cc` (padre `ev_5b8280839aa10a76e977f31f175e5e7168efc2aeda9140d298104dac5b23c0ec`)

`open_binary` lo clasificó como PE ejecutable x86_64. `inspect_managed_artifact` no encontró directorio CLI: es nativo, no un ensamblado .NET. Los recursos son tres: un recurso con nombre `MUI`, `RT_VERSION` (id 16) y `RT_MANIFEST` (id 24), idioma 1033. El manifiesto deja comentada `TaskManagerApp.dll`. La interfaz enlaza `DUI70.dll`.

## Cobertura

Observado, sin ejecutar el programa:

- Identidad del PE, manifiesto y versión.
- Tabla de imports (85 DLL, 1111 símbolos) y delay-load (28 DLL).
- Nombres de clase C++ en la RTTI (90 tipos, sin contar `type_info`).

Pendiente, porque el proveedor no está disponible en este equipo:

- Ghidra no está configurado (`GHIDRA_INSTALL_DIR` vacío). Java 17 sí está instalado.
- Hopper no corre en Windows.
- IDA no está configurado.
- Rizin y objdump responden `ERR_REA_WINDOWS_NATIVE`.

Por eso no hay pseudocódigo ni grafo de llamadas. Las conclusiones de comportamiento salen de imports y nombres de tipo, y están marcadas como inferencia.

## Arquitectura

La RTTI separa datos y vista.

### Datos (`Wdc*`)

Cada monitor alimenta listas y gráficos a través de `WdcDataPortal` y `WdcDataMonitor`.

| Clase | Rol inferido |
|---|---|
| `WdcProcessMonitor` | Lista de procesos |
| `WdcCpuMonitor`, `WdcNumaCpuList`, `WdcNumaNodeList` | CPU y topología NUMA |
| `WdcMemoryMonitor` | Memoria |
| `WdcDiskMonitor` | Disco |
| `WdcGpuMonitor` | GPU |
| `WdcNetworkMonitor` | Red |
| `WdcUserMonitor` | Sesiones y usuarios |
| `WdcServiceMonitor`, `WdcServiceCache` | Servicios |
| `WdcStartupMonitor` | Programas de inicio |
| `WdcApplicationsMonitor`, `WdcAppHistoryMonitor`, `AppHistoryStringCache` | Aplicaciones e historial de recursos |
| `WdcWindowMonitor`, `WindowHandleCrawler` | Ventanas |
| `WdcTrayIconMonitor` | Icono de bandeja |
| `WdcChart`, `WdcListView`, `WdcColumnList`, `WdcTab` | Gráfico, lista, columnas, pestañas |

### Vistas

| Prefijo | Rol inferido |
|---|---|
| `Atm*` | Panel nuevo: `AtmDashboard`, `AtmCpuView`, `AtmMemoryView`, `AtmDiskView`, `AtmGpuView`, `AtmNetworkView`, `AtmGroupHeader`, `AtmViewItem` |
| `Tm*` | Listas clásicas: `TmPage`, `TmColHeaderItem`, `TmListSorter`, `TmScrollViewer`, `TmRowTextElement`, `TmSmallView`, `TmLowMemoryView` |

La UI importa 378 símbolos de `DUI70.dll` (DirectUI: `HWNDElement`, `DUIXmlParser`, `CCListView`, proveedores de UI Automation) más `DUser.dll` y `UxTheme.dll`. Los gráficos salen de `CHARTV.dll`: `CvInitialize`, `CvCreateDataSource`, `CvSetData`, `CvCloseDataSource`, `CvUninitialize`.

## De dónde lee Windows

| Área | APIs observadas |
|---|---|
| Procesos e hilos | `NtQuerySystemInformation`, `NtQueryInformationProcess`, `NtQueryInformationThread`, `NtQueryInformationFile`, `LdrQueryProcessModuleInformation` |
| CPU, memoria, disco | `PdhOpenQueryW`, `PdhAddCounterW`, `PdhCollectQueryData`, `PerfOpenQueryHandle`, `PerfAddCounters`, `PerfQueryCounterData` |
| Red | `GetAdaptersAddresses`, `GetIfEntry2`, `NotifyUnicastIpAddressChange`, `NsiGetAllParameters` |
| GPU | `DXCoreCreateAdapterFactory`, `CreateDXGIFactory2` |
| Servicios | `OpenSCManagerW`, `EnumServicesStatusExW`, `QueryServiceStatusEx`, `ControlService`, `StartServiceW`, `SubscribeServiceChangeNotifications` |
| Sesiones | `WTSEnumerateSessionsW`, `WTSQuerySessionInformationW`, `WTSDisconnectSession`, `WTSLogoffSession`, `WinStationConnectW` |
| Usuarios | `NetUserGetInfo`, `LookupAccountSidLocalW`, `GetUserNameExW`, `ConvertSidToStringSidW` |
| Arranque y paquetes | `RegOpenKeyExW`, `RegNotifyChangeKeyValue`, `AppXGetPackageSid` |
| Energía | `NtPowerInformation`, `CallNtPowerInformation` |
| Procesos colgados | `GetThreadWaitChain`, `ReportHang` |
| Shell | `ShellExecuteExW`, `Shell_NotifyIconW`, `SHGetFileInfoW` |

## Mapa a Linux en Rust

DirectUI no se porta. La UI es propia. Cada monitor de Windows queda como un módulo que publica un snapshot.

| Monitor | Fuente en Linux | Enfoque |
|---|---|---|
| Procesos, CPU, memoria | `/proc`, `/proc/stat`, `/proc/meminfo` | `sysinfo` (en Linux lee `/proc`) |
| Disco | `/proc/diskstats`, sysfs | `sysinfo` (`Disks`) |
| Red | `/proc/net/dev` | `sysinfo` (`Networks`) |
| GPU | DRM/sysfs; NVIDIA con `nvidia-smi` | `src/gpu.rs` |
| Servicios | systemd | `systemctl list-units` y `systemctl start`/`stop` |
| Sesiones | logind | `loginctl list-sessions` |
| Arranque | `~/.config/autostart`, unidades de usuario | lectura de archivos |
| Historial de apps | sin equivalente del contador de Windows | fuera del primer corte |
| Bandeja | StatusNotifierItem | fuera del primer corte |

## Estado del puerto

El crate `taskmgr` implementa `WdcProcessMonitor`, `WdcCpuMonitor`, `WdcMemoryMonitor`, `WdcDiskMonitor`, `WdcNetworkMonitor`, `WdcGpuMonitor`, `WdcServiceMonitor` y `WdcUserMonitor`.

- `src/monitor.rs` toma CPU global, RAM, procesos, discos montados, tráfico por interfaz, servicios y sesiones.
- `src/gpu.rs` lee la utilización y la VRAM con `nvidia-smi` cuando el driver NVIDIA está instalado. En Linux, además recorre `/sys/class/drm/cardN` (porcentaje de ocupación y VRAM en AMD/Intel). En Windows enumera los adaptadores DXGI para las GPU que `nvidia-smi` no cubre; DXGI no informa el porcentaje de uso.
- `src/services.rs` lista los servicios y permite iniciarlos o detenerlos. En Linux usa `systemctl`. En Windows usa el SCM (`EnumServicesStatusExW`, `StartServiceW`, `ControlService`).
- `src/sessions.rs` lista las sesiones. En Linux usa `loginctl`. En Windows usa `WTSEnumerateSessionsW`. No cierra sesiones.
- `src/actions.rs` termina, suspende o reanuda un proceso. En Linux manda `SIGKILL`, `SIGSTOP` o `SIGCONT`. En Windows usa `TerminateProcess` y suspende o reanuda los hilos del proceso. Rechaza el propio `taskmgr`, el pid 0, el pid 1 en Linux y el pid 4 en Windows.
- La velocidad de disco y de red sale de la diferencia entre dos muestras. La primera muestra no publica tasa.
- En Linux, `sysinfo` 0.36 lee `/proc` y `/proc/net/dev`. En Windows usa las API nativas, así el mismo binario se puede compilar mientras se desarrolla.
- `taskmgr` abre la ventana con la disposición del Administrador de tareas de Windows 11: búsqueda, barra lateral y páginas de procesos agrupados, rendimiento, historial, arranque, usuarios, detalles, servicios y configuración. `taskmgr --tui` (o `-t`) abre la misma información en la terminal. `taskmgr --once` imprime un muestreo y termina.
- En la terminal: `1` a `6`, y `Tab` cambia de página.
- En procesos, el orden es CPU, memoria, nombre o PID. La CPU necesita dos muestras separadas por al menos 200 ms.
- Terminar, suspender y reanudar un proceso, e iniciar o detener un servicio, piden confirmación. En la ventana son botones. En la terminal: `x`, `z`, `r` sobre un proceso e `i`, `t` sobre un servicio; Enter confirma y Esc cancela.

Todavía no está el arranque de aplicaciones.
