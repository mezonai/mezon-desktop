#!/bin/bash

if [ ! -e "$(pwd)/Implib.so" ]
then
  git clone --depth 1 https://github.com/yugr/Implib.so.git
fi

generate_implib() {
   category=$1
   libname=$2
   arch=$3
   echo "Generating implib for category: ${category} libname: ${libname} - ${arch}, output to ${category}/${arch}/"
   mkdir -p ${category}/${arch}/
   python3 $(pwd)/Implib.so/implib-gen.py /lib/x86_64-linux-gnu/${libname}.so --target ${arch} --outdir ${category}/${arch}/
}

desktop_capturer_deps=("libdrm" "libgbm" "libXfixes" "libXdamage" "libXcomposite" "libXrandr" "libXext" "libX11")

for dep in "${desktop_capturer_deps[@]}"
do
  generate_implib "desktop_capturer" ${dep} "x86_64-linux-gnu"
  generate_implib "desktop_capturer" ${dep} "aarch64-linux-gnu"
done

nvidia_deps=("libcuda" "libnvcuvid")

for dep in "${nvidia_deps[@]}"
do
  generate_implib "nvidia" ${dep} "x86_64-linux-gnu"
  generate_implib "nvidia" ${dep} "aarch64-linux-gnu"
done


vaapi_deps=("libva" "libva-drm")
for dep in "${vaapi_deps[@]}"
do
  generate_implib "vaapi" ${dep} "x86_64-linux-gnu"
  generate_implib "vaapi" ${dep} "aarch64-linux-gnu"
done