// The TAP-Windows driver's interface, as far as the OpenVPN 3 core's Windows
// code refers to it (openvpn/tun/win/tunutil.hpp, reached through
// openvpn/netconf/hwaddr.hpp). uniray-ovpn never opens a TAP device: its
// packets go to the engine (see uniray-ovpn.cpp), so these only let the core
// compile. The values are the driver's documented control codes and registry
// paths. TAP_WIN_COMPONENT_ID comes from the core's CMake.
#ifndef UNIRAY_TAP_WINDOWS_H
#define UNIRAY_TAP_WINDOWS_H

#include <windows.h>
#include <winioctl.h>

#define TAP_WIN_CONTROL_CODE(request, method) CTL_CODE(FILE_DEVICE_UNKNOWN, request, method, FILE_ANY_ACCESS)

#define TAP_WIN_IOCTL_GET_MAC TAP_WIN_CONTROL_CODE(1, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_GET_VERSION TAP_WIN_CONTROL_CODE(2, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_GET_MTU TAP_WIN_CONTROL_CODE(3, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_GET_INFO TAP_WIN_CONTROL_CODE(4, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_CONFIG_POINT_TO_POINT TAP_WIN_CONTROL_CODE(5, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_SET_MEDIA_STATUS TAP_WIN_CONTROL_CODE(6, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_CONFIG_DHCP_MASQ TAP_WIN_CONTROL_CODE(7, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_GET_LOG_LINE TAP_WIN_CONTROL_CODE(8, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_CONFIG_DHCP_SET_OPT TAP_WIN_CONTROL_CODE(9, METHOD_BUFFERED)
#define TAP_WIN_IOCTL_CONFIG_TUN TAP_WIN_CONTROL_CODE(10, METHOD_BUFFERED)

#define ADAPTER_KEY "SYSTEM\\CurrentControlSet\\Control\\Class\\{4D36E972-E325-11CE-BFC1-08002BE10318}"
#define NETWORK_CONNECTIONS_KEY "SYSTEM\\CurrentControlSet\\Control\\Network\\{4D36E972-E325-11CE-BFC1-08002BE10318}"

#define USERMODEDEVICEDIR "\\\\.\\Global\\"
#define SYSDEVICEDIR "\\Device\\"
#define USERDEVICEDIR "\\DosDevices\\Global\\"
#define TAP_WIN_SUFFIX ".tap"

#endif
