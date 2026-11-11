cmake_minimum_required(VERSION 3.28)
project(weebshot_anime4k LANGUAGES CXX)

include(FetchContent)
set(AC_BUILD_CLI OFF CACHE BOOL "")
set(AC_CORE_WITH_OPENCL ON CACHE BOOL "")
if(WIN32)
    set(AC_ENABLE_STATIC_CRT ON CACHE BOOL "")
endif()
# Anime4KCPP 3.2 expects stb_image_resize2.h at the repository root.
FetchContent_Declare(stb
    GIT_REPOSITORY https://github.com/nothings/stb.git
    GIT_TAG 904aa67e1e2d1dec92959df63e700b166d5c1022)
FetchContent_Declare(anime4kcpp
    GIT_REPOSITORY https://github.com/TianZerL/Anime4KCPP.git
    GIT_TAG 50c5d1b99965d804eeecfab9b48acfcaffddee3d # v3.2.0
    EXCLUDE_FROM_ALL)
FetchContent_MakeAvailable(anime4kcpp)

# The upstream OpenCL processor accepts CPU devices too. Restrict it to GPUs
# for this helper, leaving the dedicated CPU backend available everywhere.
if(AC_CORE_WITH_OPENCL)
    set(opencl_source "${anime4kcpp_SOURCE_DIR}/core/src/processor/opencl/OpenCLProcessor.cpp")
    file(READ "${opencl_source}" opencl_code)
    string(REPLACE "platform.getDevices(CL_DEVICE_TYPE_ALL, &devices)"
        "platform.getDevices(CL_DEVICE_TYPE_GPU, &devices)" gpu_code "${opencl_code}")
    if(gpu_code STREQUAL opencl_code)
        message(FATAL_ERROR "Could not restrict Anime4KCPP OpenCL enumeration to GPUs")
    endif()
    file(CONFIGURE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/OpenCLProcessor.cpp" CONTENT "${gpu_code}" @ONLY)
    get_target_property(core_sources ac SOURCES)
    list(TRANSFORM core_sources REPLACE "${opencl_source}" "${CMAKE_CURRENT_BINARY_DIR}/OpenCLProcessor.cpp")
    set_property(TARGET ac PROPERTY SOURCES "${core_sources}")
endif()

add_executable(weebshot-anime4k main.cpp)
target_compile_features(weebshot-anime4k PRIVATE cxx_std_17)
target_link_libraries(weebshot-anime4k PRIVATE AC::Core)
if(MINGW)
    target_link_options(weebshot-anime4k PRIVATE -municode)
endif()

if(WIN32)
    target_compile_definitions(ac PRIVATE STBI_WINDOWS_UTF8 STBIW_WINDOWS_UTF8)
    set_property(TARGET weebshot-anime4k PROPERTY MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
    set(FFMPEG_SDK "" CACHE PATH "FFmpeg shared build with headers and import libraries")
    target_include_directories(weebshot-anime4k PRIVATE "${FFMPEG_SDK}/include")
    foreach(lib avformat avcodec avutil)
        find_library(${lib}_LIBRARY NAMES ${lib} PATHS "${FFMPEG_SDK}/lib" NO_DEFAULT_PATH REQUIRED)
        target_link_libraries(weebshot-anime4k PRIVATE "${${lib}_LIBRARY}")
    endforeach()
else()
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(AV REQUIRED IMPORTED_TARGET libavformat libavcodec libavutil)
    target_link_libraries(weebshot-anime4k PRIVATE PkgConfig::AV)
endif()

install(TARGETS weebshot-anime4k RUNTIME DESTINATION bin)
if(WIN32 AND TARGET OpenCL)
    install(FILES $<TARGET_FILE:OpenCL> DESTINATION bin)
endif()
install(FILES "${anime4kcpp_SOURCE_DIR}/LICENSE-MIT" DESTINATION licenses RENAME Anime4KCPP-MIT.txt)
foreach(dependency stb ruapu half opencl openclheaders openclheaderscpp)
    FetchContent_GetProperties(${dependency})
    foreach(notice LICENSE LICENSE.txt)
        if(EXISTS "${${dependency}_SOURCE_DIR}/${notice}")
            install(FILES "${${dependency}_SOURCE_DIR}/${notice}"
                DESTINATION licenses RENAME "${dependency}-LICENSE.txt")
            break()
        endif()
    endforeach()
endforeach()
foreach(dependency OpenCL-Headers OpenCL-ICD-Loader OpenCL-CLHPP)
    foreach(notice LICENSE LICENSE.txt)
        if(EXISTS "${opencl_SOURCE_DIR}/external/${dependency}/${notice}")
            install(FILES "${opencl_SOURCE_DIR}/external/${dependency}/${notice}"
                DESTINATION licenses RENAME "${dependency}-LICENSE.txt")
            break()
        endif()
    endforeach()
endforeach()
