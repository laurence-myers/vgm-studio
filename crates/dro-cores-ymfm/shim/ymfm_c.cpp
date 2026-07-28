// SPDX-License-Identifier: MIT OR Apache-2.0
//
// A C surface over ymfm's C++ chips.
//
// ymfm (BSD-3-Clause, Aaron Giles) is C++14 and Rust cannot call C++
// directly, so this file is the whole of the bridge: an opaque handle, a
// virtual base that erases the chip type, and one template that adapts any
// ymfm chip to it. Adding a chip is a line in `ymfm_create` -- which is the
// point of reusing a library that already presents every chip the same way.
//
// Nothing here patches the upstream; it is compiled unmodified, exactly as
// the Nuked submodules are. See CORES-REUSE-PLAN.md §4.
//
// The interface object: ymfm chips take a `ymfm_interface&` for timers, IRQ
// and external memory. Timer callbacks are left at their upstream defaults,
// which is what `examples/vgmrender/vgmrender.cpp` does -- a VGM is a log of
// writes, so nothing in the stream is waiting on a timer flag to be read
// back. External memory is served from buffers the caller loads, which is
// how ADPCM and PCM sample ROMs arrive.

#include <cstdint>
#include <cstring>
#include <vector>

#include "ymfm_opn.h"

namespace {

/// The chip kinds this shim can build. Kept in sync with the Rust side's
/// `Kind`; a mismatch is a null handle, never a wrong chip.
enum kind_t : int {
    KIND_YM2203 = 0,
    KIND_YM2608 = 1,
    KIND_YM2610 = 2,
    KIND_YM2610B = 3,
    KIND_YM2612 = 4,
    KIND_YM3438 = 5,
};

/// How many of a chip's outputs are the FM section.
///
/// ymfm gives each part its natural channel count and they genuinely
/// differ: the YM2612 has two outputs and no SSG at all; the YM2608 and
/// YM2610 have stereo FM plus a **pre-summed** mono SSG (their own
/// `SSG_OUTPUTS` is 1); the YM2203 has *mono* FM plus the SSG's **three
/// separate** channels. So the FM width cannot be inferred -- only the
/// chips with an SSG even declare `FM_OUTPUTS` -- and this trait states it,
/// deferring to the chip's own constant wherever there is one so an
/// upstream change cannot silently desynchronise it.
template <typename ChipType> struct chip_layout {
    // No SSG: every output is FM.
    static constexpr uint32_t FM = ChipType::OUTPUTS;
};
template <> struct chip_layout<ymfm::ym2203> {
    static constexpr uint32_t FM = ymfm::ym2203::FM_OUTPUTS;
};
template <> struct chip_layout<ymfm::ym2608> {
    static constexpr uint32_t FM = ymfm::ym2608::FM_OUTPUTS;
};
template <> struct chip_layout<ymfm::ym2610> {
    static constexpr uint32_t FM = ymfm::ym2610::FM_OUTPUTS;
};
template <> struct chip_layout<ymfm::ym2610b> {
    static constexpr uint32_t FM = ymfm::ym2610b::FM_OUTPUTS;
};

/// Type-erased chip, so the C surface does not need a template.
struct chip_base {
    virtual ~chip_base() = default;
    virtual void reset() = 0;
    virtual uint32_t sample_rate(uint32_t clock) const = 0;
    virtual void write(uint32_t offset, uint8_t data) = 0;
    virtual void generate(int32_t *out, uint32_t frames) = 0;
    virtual void load(int access, uint32_t offset, const uint8_t *data,
                      uint32_t len) = 0;
};

/// Adapts one ymfm chip to `chip_base`, and serves its external memory.
template <typename ChipType>
class chip_impl final : public chip_base, public ymfm::ymfm_interface {
public:
    explicit chip_impl(uint32_t clock) : m_chip(*this), m_clock(clock) {
        m_chip.reset();
    }

    void reset() override { m_chip.reset(); }

    uint32_t sample_rate(uint32_t clock) const override {
        return m_chip.sample_rate(clock);
    }

    void write(uint32_t offset, uint8_t data) override {
        m_chip.write(offset, data);
    }

    /// Renders `frames` stereo frames, folding the chip's own output layout
    /// down to a pair.
    ///
    /// The FM section comes first (see [`chip_layout`]) and whatever follows
    /// it is SSG, which has no stereo position on any of these parts and so
    /// is added to both sides -- the same arrangement the LLE wrapper uses,
    /// and the reason a 2608's three ymfm outputs become two here.
    void generate(int32_t *out, uint32_t frames) override {
        constexpr uint32_t FM = chip_layout<ChipType>::FM;
        constexpr uint32_t ALL = ChipType::OUTPUTS;
        typename ChipType::output_data frame;
        for (uint32_t i = 0; i < frames; i++) {
            m_chip.generate(&frame, 1);
            int32_t left = frame.data[0];
            int32_t right = frame.data[FM > 1 ? 1 : 0];
            // Whatever follows the FM block is the SSG, mono into both.
            for (uint32_t ch = FM; ch < ALL; ch++) {
                left += frame.data[ch];
                right += frame.data[ch];
            }
            out[i * 2 + 0] = left;
            out[i * 2 + 1] = right;
        }
    }

    /// Fills one of the chip's external memory spaces -- ADPCM-A, ADPCM-B or
    /// PCM sample ROM -- at `offset`, growing the buffer as pieces arrive.
    /// A VGM delivers a ROM in fragments, so this must accumulate.
    void load(int access, uint32_t offset, const uint8_t *data,
              uint32_t len) override {
        if (access < 0 || access >= ymfm::ACCESS_CLASSES) {
            return;
        }
        auto &space = m_data[access];
        size_t end = static_cast<size_t>(offset) + len;
        if (space.size() < end) {
            space.resize(end, 0);
        }
        std::memcpy(space.data() + offset, data, len);
    }

    /// ymfm asks here for every ADPCM and PCM sample byte.
    uint8_t ymfm_external_read(ymfm::access_class type,
                               uint32_t address) override {
        auto &space = m_data[type];
        return (address < space.size()) ? space[address] : 0;
    }

private:
    ChipType m_chip;
    uint32_t m_clock;
    std::vector<uint8_t> m_data[ymfm::ACCESS_CLASSES];
};

} // namespace

extern "C" {

/// Builds a chip, or returns null for a kind this shim does not know.
chip_base *ymfm_create(int kind, uint32_t clock) {
    switch (kind) {
    case KIND_YM2203:
        return new chip_impl<ymfm::ym2203>(clock);
    case KIND_YM2608:
        return new chip_impl<ymfm::ym2608>(clock);
    case KIND_YM2610:
        return new chip_impl<ymfm::ym2610>(clock);
    case KIND_YM2610B:
        return new chip_impl<ymfm::ym2610b>(clock);
    case KIND_YM2612:
        return new chip_impl<ymfm::ym2612>(clock);
    case KIND_YM3438:
        return new chip_impl<ymfm::ym3438>(clock);
    default:
        return nullptr;
    }
}

void ymfm_destroy(chip_base *chip) { delete chip; }

void ymfm_reset(chip_base *chip) { chip->reset(); }

uint32_t ymfm_sample_rate(const chip_base *chip, uint32_t clock) {
    return chip->sample_rate(clock);
}

void ymfm_write(chip_base *chip, uint32_t offset, uint8_t data) {
    chip->write(offset, data);
}

void ymfm_generate(chip_base *chip, int32_t *out, uint32_t frames) {
    chip->generate(out, frames);
}

void ymfm_load_data(chip_base *chip, int access, uint32_t offset,
                    const uint8_t *data, uint32_t len) {
    chip->load(access, offset, data, len);
}

} // extern "C"
