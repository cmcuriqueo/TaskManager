# taskmgr

Administrador de tareas escrito en Rust. Abre una ventana con la disposición del Administrador de tareas de Windows 11 y, con otro flag, la misma información en la terminal.

El muestreo sale de las fuentes del sistema: en Linux de `/proc`, systemd y logind; en Windows de las API nativas. No es el binario de Microsoft.

## Funcionalidades

Tres modos:

| Comando | Qué hace |
|---|---|
| `taskmgr` | Ventana |
| `taskmgr --tui` | Vista en la terminal (`-t`) |
| `taskmgr --once` | Imprime un muestreo y termina |

La ventana tiene búsqueda y estas páginas:

- **Procesos.** Agrupados por nombre, con CPU, memoria y disco. Se expanden para ver cada PID.
- **Rendimiento.** CPU, memoria, discos, interfaces de red y GPU, con un gráfico de los últimos 60 segundos.
- **Historial de aplicaciones.** Tiempo de CPU acumulado desde que se abrió la ventana.
- **Aplicaciones de arranque.** Entradas de inicio del usuario y del sistema.
- **Usuarios.** Procesos agrupados por usuario.
- **Detalles.** Lista plana con PID, estado y usuario.
- **Servicios.** Lista, inicio y detención.
- **Configuración.** Notas de este corte.

Desde la ventana se puede finalizar, suspender o reanudar un proceso, iniciar o detener un servicio, y lanzar un programa con **Ejecutar nueva tarea**. Cada acción pide confirmación.

La terminal muestra procesos, discos, red, GPU, servicios y sesiones. El orden de procesos es por CPU, memoria, nombre o PID. La CPU necesita dos muestras separadas por al menos 200 ms; la primera lectura de disco y de red no publica tasa.

Límites de las acciones sobre procesos: no actúa sobre sí mismo, ni sobre el pid 0, ni sobre el pid 1 en Linux, ni sobre el pid 4 en Windows.

## Compatibilidad

| | Linux | Windows |
|---|---|---|
| CPU, memoria, procesos, discos, red | `/proc`, `/proc/net/dev` vía `sysinfo` | API nativas vía `sysinfo` |
| GPU | `nvidia-smi` y `/sys/class/drm` (AMD e Intel: uso y VRAM cuando el driver los publica) | `nvidia-smi`; el resto de adaptadores por DXGI (nombre y memoria total, sin porcentaje de uso) |
| Servicios | `systemctl` | SCM (`EnumServicesStatusExW`, `StartServiceW`, `ControlService`) |
| Sesiones | `loginctl` (solo lectura) | `WTSEnumerateSessionsW` (solo lectura) |
| Arranque | `~/.config/autostart` y `/etc/xdg/autostart` | `Run` de HKCU y HKLM, más las carpetas de inicio |
| Iconos y tipografía | Iconos genéricos. La fuente es la de egui | Iconos del shell y Segoe UI si está en `C:\Windows\Fonts` |

macOS no está soportado. El crate puede llegar a compilar, pero servicios, sesiones y GPU de plataforma quedan vacíos.

## Requisitos

- Rust 1.85 o posterior (el crate usa edition 2024).
- Un escritorio para la ventana. `--tui` y `--once` no lo necesitan.
- En Linux, `systemctl` y `loginctl` para servicios y sesiones. `nvidia-smi` es opcional.
- Iniciar o detener un servicio, o actuar sobre un proceso de otro usuario, pide privilegios del sistema (administrador en Windows, polkit o root en Linux).

## Uso

```sh
cargo build --release
cargo run --release
cargo run --release -- --tui
cargo run --release -- --once
cargo test
```

El binario queda en `target/release/taskmgr`.

```text
taskmgr [--tui] [--once]
  (sin flags)   abre la ventana
  -t, --tui     abre la vista en la terminal
  --once        imprime un muestreo y termina
  -h, --help    muestra la ayuda
```

`--tui` y `--once` no se combinan.

### Terminal

| Tecla | Acción |
|---|---|
| `q`, `Esc` | Salir |
| `1`–`6`, `Tab`, `Shift+Tab` | Cambiar de página |
| `↑` `↓`, `j` `k` | Mover la selección |
| `c` `m` `n` `p` | Ordenar procesos por CPU, memoria, nombre o PID |
| `x` `z` `r` | Terminar, suspender o reanudar el proceso seleccionado |
| `i` `t` | Iniciar o detener el servicio seleccionado |
| `Enter` / `Esc` | Confirmar o cancelar la acción pendiente |

## Por hacer

- Historial de red y de notificaciones. Esas columnas quedan en `—`. El tiempo de CPU solo cuenta desde que se abrió la ventana.
- Impacto de inicio. La columna existe y siempre muestra `—`. Tampoco se habilita ni se deshabilita una entrada de arranque desde la interfaz.
- Cerrar o desconectar una sesión.
- Icono de bandeja.
- Porcentaje de uso de GPU en Windows cuando no hay `nvidia-smi`.
- Iconos de archivo en Linux.
- Vista de topología NUMA.

## Estructura

| Módulo | Rol |
|---|---|
| `src/main.rs` | Argumentos y modo `--once` |
| `src/gui.rs` | Ventana |
| `src/tui.rs` | Terminal |
| `src/monitor.rs` | CPU, memoria, procesos, discos y red |
| `src/gpu.rs` | Adaptadores |
| `src/services.rs` | Servicios |
| `src/sessions.rs` | Sesiones |
| `src/startup.rs` | Programas de arranque |
| `src/actions.rs` | Terminar, suspender y reanudar |
| `src/icons.rs` | Iconos de la ventana |
