#include <AC/Core/Image.hpp>
#include <AC/Core/Processor.hpp>

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
}

#include <algorithm>
#include <iostream>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#endif

using ac::core::Image;
using ac::core::Processor;

static int checked(int result, const char* action)
{
    if (result < 0) {
        char error[AV_ERROR_MAX_STRING_SIZE]{};
        av_strerror(result, error, sizeof(error));
        throw std::runtime_error(std::string(action) + ": " + error);
    }
    return result;
}

template<typename T, void (*Free)(T**)>
static auto owned(T* value)
{
    if (!value) throw std::bad_alloc();
    return std::unique_ptr<T, void (*)(T*)>(value, [](T* ptr) { Free(&ptr); });
}

static void close_output(AVFormatContext* output)
{
    if (output->pb) avio_closep(&output->pb);
    avformat_free_context(output);
}

static std::shared_ptr<Processor> select_processor(
    const std::string& device, const char* model, Image::ElementType type)
{
    if (device != "auto" && device != "cpu" && device != "gpu")
        throw std::runtime_error("Expected auto, gpu, or cpu");

    std::string reason = "no compatible OpenCL GPU found";
#ifdef AC_CORE_WITH_OPENCL
    if (device != "cpu") {
        const std::string info = Processor::info<Processor::OpenCL>();
        const auto count = std::count(info.begin(), info.end(), '\n') - 1;
        // Try the upstream performance-ranked default first, then the remaining devices.
        for (int index = -1; count > 0 && index < count; ++index) {
            auto gpu = Processor::create("opencl", index, model);
            if (gpu && gpu->ok()) {
                Image probe(16, 16, 1, type);
                std::fill_n(probe.data(), probe.size(), 0);
                auto result = gpu->process(probe, 2.0);
                if (gpu->ok() && !result.empty()) {
                    std::cout << "backend=gpu" << std::endl;
                    return gpu;
                }
            }
            if (gpu && gpu->error()) reason = gpu->error();
        }
    }
#endif
    if (device == "gpu") {
        throw std::runtime_error("OpenCL GPU unavailable (" + reason +
            "). Install or update your GPU's OpenCL driver, or choose Auto or CPU.");
    }
    auto cpu = Processor::create("cpu", -1, model);
    if (!cpu || !cpu->ok()) throw std::runtime_error("Anime4K CPU initialization failed");
    std::cout << "backend=cpu" << std::endl;
    return cpu;
}

static void upscale(Processor& processor, const Image& input, Image& output)
{
    processor.process(input, output, 2.0);
    if (!processor.ok() || output.empty())
        throw std::runtime_error(std::string("Anime4K processing failed: ") +
            (processor.error() ? processor.error() : "empty frame"));
}

static void image(const std::string& device, const char* model, const char* input, const char* output)
{
    auto frame = ac::core::imread(input, 3);
    if (frame.empty()) throw std::runtime_error("Could not read lossless screenshot");
    auto processor = select_processor(device, model, frame.type());
    Image result;
    upscale(*processor, frame, result);
    if (!ac::core::imwrite(output, result))
        throw std::runtime_error("Could not write upscaled screenshot");
}

static void video(const std::string& device, const char* model, const char* input, const char* output)
{
    AVFormatContext* input_context = nullptr;
    checked(avformat_open_input(&input_context, input, nullptr, nullptr), "Open prepared video");
    auto source = owned<AVFormatContext, avformat_close_input>(input_context);
    checked(avformat_find_stream_info(source.get(), nullptr), "Read prepared stream");
    const AVCodec* decoder = nullptr;
    const int index = checked(av_find_best_stream(source.get(), AVMEDIA_TYPE_VIDEO, -1, -1, &decoder, 0),
        "Find prepared video stream");
    const auto* stream = source->streams[index];
    if (decoder->id != AV_CODEC_ID_FFV1 || stream->codecpar->format != AV_PIX_FMT_YUV444P16LE)
        throw std::runtime_error("Expected prepared FFV1 video in yuv444p16le");

    auto decode = owned<AVCodecContext, avcodec_free_context>(avcodec_alloc_context3(decoder));
    checked(avcodec_parameters_to_context(decode.get(), stream->codecpar), "Set decoder parameters");
    decode->pkt_timebase = stream->time_base;
    checked(avcodec_open2(decode.get(), decoder, nullptr), "Open FFV1 decoder");
    if (decode->width < 1 || decode->height < 1 ||
        decode->width > std::numeric_limits<int>::max() / 2 ||
        decode->height > std::numeric_limits<int>::max() / 2)
        throw std::runtime_error("Invalid prepared frame dimensions");

    auto processor = select_processor(device, model, Image::UInt16);
    const auto* encoder = avcodec_find_encoder(AV_CODEC_ID_FFV1);
    if (!encoder) throw std::runtime_error("FFV1 encoder is unavailable");
    auto encode = owned<AVCodecContext, avcodec_free_context>(avcodec_alloc_context3(encoder));
    checked(avcodec_parameters_to_context(encode.get(), stream->codecpar), "Copy video parameters");
    // Decoder extradata belongs to the original stream.
    av_freep(&encode->extradata);
    encode->extradata_size = 0;
    encode->width *= 2;
    encode->height *= 2;
    encode->pix_fmt = AV_PIX_FMT_YUV444P16LE;
    encode->time_base = stream->time_base;
    encode->framerate = stream->avg_frame_rate;
    encode->level = 3;
    encode->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
    checked(avcodec_open2(encode.get(), encoder, nullptr), "Open FFV1 encoder");

    AVFormatContext* output_context = nullptr;
    checked(avformat_alloc_output_context2(&output_context, nullptr, "matroska", output),
        "Create upscaled video");
    std::unique_ptr<AVFormatContext, decltype(&close_output)> destination(output_context, close_output);
    auto* track = avformat_new_stream(destination.get(), nullptr);
    if (!track) throw std::bad_alloc();
    track->time_base = encode->time_base;
    track->avg_frame_rate = stream->avg_frame_rate;
    track->sample_aspect_ratio = stream->sample_aspect_ratio;
    checked(avcodec_parameters_from_context(track->codecpar, encode.get()), "Write video parameters");
    checked(avio_open(&destination->pb, output, AVIO_FLAG_WRITE), "Open upscaled video");
    checked(avformat_write_header(destination.get(), nullptr), "Write Matroska header");

    auto packet = owned<AVPacket, av_packet_free>(av_packet_alloc());
    auto encoded = owned<AVPacket, av_packet_free>(av_packet_alloc());
    auto frame = owned<AVFrame, av_frame_free>(av_frame_alloc());
    auto doubled = owned<AVFrame, av_frame_free>(av_frame_alloc());
    doubled->format = encode->pix_fmt;
    doubled->width = encode->width;
    doubled->height = encode->height;
    checked(av_frame_get_buffer(doubled.get(), 0), "Allocate upscaled frame");

    const auto write_packets = [&](AVFrame* next) {
        checked(avcodec_send_frame(encode.get(), next), "Send FFV1 frame");
        for (;;) {
            int result = avcodec_receive_packet(encode.get(), encoded.get());
            if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) break;
            checked(result, "Encode FFV1 frame");
            av_packet_rescale_ts(encoded.get(), encode->time_base, track->time_base);
            encoded->stream_index = track->index;
            checked(av_interleaved_write_frame(destination.get(), encoded.get()), "Write FFV1 packet");
        }
    };

    std::size_t frames = 0;
    const auto decode_frames = [&](AVPacket* next) {
        checked(avcodec_send_packet(decode.get(), next), "Send FFV1 packet");
        for (;;) {
            int result = avcodec_receive_frame(decode.get(), frame.get());
            if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) break;
            checked(result, "Decode FFV1 frame");
            if (frame->width != decode->width || frame->height != decode->height ||
                frame->width * 2 != doubled->width || frame->height * 2 != doubled->height ||
                frame->format != doubled->format)
                throw std::runtime_error("Prepared frame format changed mid-clip");
            checked(av_frame_make_writable(doubled.get()), "Reuse upscaled frame");
            for (int plane = 0; plane < 3; ++plane) {
                const Image src(frame->width, frame->height, 1, Image::UInt16,
                    frame->data[plane], frame->linesize[plane]);
                Image dst(doubled->width, doubled->height, 1, Image::UInt16,
                    doubled->data[plane], doubled->linesize[plane]);
                if (plane == 0) upscale(*processor, src, dst);
                else ac::core::resize(src, dst, 2.0, 2.0);
            }
            // Keep frame-level color/HDR side data as well as stream metadata.
            while (doubled->nb_side_data > 0)
                av_frame_remove_side_data(doubled.get(), doubled->side_data[0]->type);
            checked(av_frame_copy_props(doubled.get(), frame.get()), "Copy frame metadata");
            doubled->pts = frame->best_effort_timestamp;
            if (doubled->pts == AV_NOPTS_VALUE)
                throw std::runtime_error("Missing prepared frame timestamp");
            if (frames == 0) {
                std::cout << "video_start_us="
                          << av_rescale_q(doubled->pts, stream->time_base, AV_TIME_BASE_Q)
                          << std::endl;
            }
            write_packets(doubled.get());
            ++frames;
            std::cout << "out_time_us=" << av_rescale_q(doubled->pts + doubled->duration,
                stream->time_base, AV_TIME_BASE_Q) << std::endl;
            av_frame_unref(frame.get());
        }
    };

    int status;
    while ((status = av_read_frame(source.get(), packet.get())) >= 0) {
        if (packet->stream_index == index) decode_frames(packet.get());
        av_packet_unref(packet.get());
    }
    if (status != AVERROR_EOF) checked(status, "Read prepared packet");
    decode_frames(nullptr);
    if (frames == 0) throw std::runtime_error("No frames in the selected range");
    write_packets(nullptr);
    checked(av_write_trailer(destination.get()), "Finish upscaled video");
    checked(avio_closep(&destination->pb), "Close upscaled video");
}

static int run(int argc, char** argv)
{
    try {
        if (argc == 3 && std::string(argv[1]) == "probe") {
            auto processor = select_processor(argv[2], "acnet-f8b8", Image::UInt16);
            Image input(16, 16, 1, Image::UInt16);
            std::fill_n(input.data(), input.size(), 0);
            Image output;
            upscale(*processor, input, output);
            if (output.width() != 32 || output.height() != 32)
                throw std::runtime_error("Unexpected probe frame dimensions");
        } else if (argc == 6 && std::string(argv[1]) == "image") {
            image(argv[2], argv[3], argv[4], argv[5]);
        } else if (argc == 6 && std::string(argv[1]) == "video") {
            video(argv[2], argv[3], argv[4], argv[5]);
        } else {
            throw std::runtime_error("Usage: weebshot-anime4k probe DEVICE | image|video DEVICE MODEL INPUT OUTPUT");
        }
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv)
{
    std::vector<std::string> utf8(argc);
    std::vector<char*> args(argc);
    for (int i = 0; i < argc; ++i) {
        const int length = WideCharToMultiByte(CP_UTF8, 0, argv[i], -1, nullptr, 0, nullptr, nullptr);
        if (length < 1) return 1;
        utf8[i].resize(length);
        WideCharToMultiByte(CP_UTF8, 0, argv[i], -1, utf8[i].data(), length, nullptr, nullptr);
        args[i] = utf8[i].data();
    }
    return run(argc, args.data());
}
#else
int main(int argc, char** argv)
{
    return run(argc, argv);
}
#endif
