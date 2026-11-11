# uniray-ovpn, the OpenVPN client behind uniray-engine (see uniray-ovpn.cpp),
# on the OpenVPN 3 core. scripts/build-ovpn.sh fetches the core at its pinned
# commit and builds this with -DOPENVPN3_DIR=<core source>.
cmake_minimum_required(VERSION 3.13...3.28)
project(uniray-ovpn CXX C)

if (NOT OPENVPN3_DIR)
    message(FATAL_ERROR "set OPENVPN3_DIR to the OpenVPN 3 core source")
endif ()
list(APPEND CMAKE_MODULE_PATH ${OPENVPN3_DIR}/cmake)
include(findcoredeps)

add_executable(uniray-ovpn uniray-ovpn.cpp)
add_core_dependencies(uniray-ovpn)
