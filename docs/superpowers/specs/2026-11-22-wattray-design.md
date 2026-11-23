# wattray — diseño

Mini app de Windows que muestra el % de batería de los periféricos inalámbricos en la bandeja del sistema. Objetivo número uno: consumo mínimo (binario chico, RAM ~1 MB, CPU ~0 entre consultas). Pensada para distribuirse: no se ata a los dispositivos de una sola persona.

## Decisiones

| Tema | Decisión | Por qué |
|---|---|---|
| Lenguaje | Rust + `windows-sys`, Win32 directo | Nativo, sin runtime ni GC; sin frameworks de UI |
| Display | Un ícono de bandeja por dispositivo, número dibujado adentro | Único camino nativo en Win11 (no hay texto libre en la barra) |
| Refresco | Timer de 2 min + reescaneo al conectar/desconectar (`WM_DEVICECHANGE`, debounce 2 s) | La batería cambia lento; reaccionar a conexiones da buena UX |
| Inicio | Se registra en `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` al arrancar | Sin instalador; se desactiva desde Administrador de tareas → Inicio |
| Menú | Click derecho → "Salir" | Nada más hace falta |
| Distribución | Repo público `facuga7van/wattray`, MIT, `.exe` en GitHub Releases (Actions al pushear tag `v*`) | Gratis; exe sin firmar → aviso de SmartScreen (aceptado) |

## Soporte de dispositivos (v1)

Lectores genéricos por protocolo, no por modelo, salvo donde el protocolo lo obliga:

1. **Bluetooth (cualquier marca).** Propiedad `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2` en devnodes `BTHLE\DEV_*` y `BTHENUM\DEV_*`, solo si están conectados. Windows ya hace el trabajo.
2. **Logitech HID++ 2.0 (todo Logitech).** Cualquier interfaz HID con VID `046D` y usage page `0xFF00` (reportes HID++ short/long). Receptores Lightspeed/Unifying/Bolt: slots 1–6. Dispositivos directos (headsets G, cable): índice `0xFF`. Feature `0x1004` Unified Battery → `0x1000` Battery Status → `0x1001` Battery Voltage (voltaje → % con curva). Nombre vía feature `0x0005` Device Name.
3. **SteelSeries Arctis.** Tabla de modelos (PID, interfaz, bytes de pedido, offset y escala de la respuesta). Arranca con Nova Pro Wireless y la familia Arctis inalámbrica conocida.
4. **Razer.** Protocolo de feature report de 90 bytes, comando de batería (clase `0x07`, id `0x80`), probando los transaction ids conocidos. Cubre la mayoría de mouse/teclados inalámbricos Razer.

Fuera de v1: Corsair (depende de iCUE), HyperX (protocolo distinto por modelo), dongles 2.4 G genéricos sin documentar. La tabla y los lectores quedan armados para sumar después.

Referencia de protocolos: HeadsetControl, Solaar, OpenRazer. Son GPL: se toman los **datos del protocolo** (bytes, offsets), el código se escribe de cero.

## Arquitectura

```
src/
  main.rs        ventana oculta, loop de mensajes, timer, WM_DEVICECHANGE, TaskbarCreated, Run key, --once
  tray.rs        dibujo del ícono (GDI → HICON) y alta/baja/cambio con Shell_NotifyIconW
  hid.rs         capa fina Win32 HID: enumerar por VID/usage, abrir, escribir/leer con timeout, feature reports
  readers/
    bt.rs        Bluetooth vía CM_Get_DevNode_PropertyW
    hidpp.rs     Logitech
    steelseries.rs
    razer.rs
```

Contrato de cada lector: `fn read(out: &mut Vec<Reading>)`, con `Reading { id: String, name: String, percent: u8, charging: bool }`. `id` es estable (VID:PID:slot o dirección BT) y de ahí sale el `uID` del ícono. Un lector que falla no agrega nada: sin panics, sin popups.

## Ícono

- Tamaño `SM_CXSMICON` (respeta DPI). Número en negrita centrado; `100` se muestra como `F`.
- Color: blanco (barra oscura) o negro (barra clara, leído de `SystemUsesLightTheme`); ámbar < 20 %, rojo < 10 %.
- Tooltip: `Nombre — 30%` (+ ` · cargando`).
- Diff por `id` en cada refresco: alta de los nuevos, baja de los que desaparecieron, modificación solo si cambió el valor.
- Mensaje `TaskbarCreated`: si explorer se reinicia, se vuelven a agregar todos los íconos.

## Testing

- `wattray.exe --once`: abre una consola, imprime las lecturas y sale. Sirve para validar contra GG/G HUB/Synapse.
- Tests unitarios chicos: escala → %, voltaje → %, texto del ícono (`100` → `F`), parseo de respuestas HID++/Razer con buffers fijos.

## Riesgos

- Software del fabricante (GG, G HUB, Synapse) abierto en simultáneo: las colecciones vendor HID suelen compartirse, pero puede haber respuestas cruzadas. Se filtra cada respuesta por id de feature/función y por software id propio.
- Offsets de SteelSeries por modelo: se validan con `--once` en el hardware disponible (Nova Pro); el resto de la tabla queda "según referencia, sin probar".
