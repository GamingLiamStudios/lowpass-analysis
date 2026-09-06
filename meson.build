project('lowpass_analysis', 'cpp', default_options: ['cpp_std=c++20'])

vulkan = dependency('vulkan', required: true, version: '>=1.3')
libplacebo = dependency('libplacebo', required: true, version: ['>=7.360', '<8'])

# ffmpeg 9.0
avutil = dependency('libavutil', required: true, version: ['>=61', '<62'])
avcodec = dependency('libavcodec', required: true, version: ['>=63', '<64'])
avformat = dependency('libavformat', required: true, version: ['>=63', '<64'])

sources = files(
    'src/main.cpp'
)
executable('lowpass_analysis', sources,
    dependencies: [vulkan, libplacebo, avutil, avcodec, avformat]
)