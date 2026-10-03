#pragma once

#include <cstddef>

class IndexWindow {
public:
    IndexWindow(std::size_t begin, std::size_t end) : begin_(begin), end_(end) {}

    bool contains(std::size_t position) const;

private:
    std::size_t begin_;
    std::size_t end_;
};

