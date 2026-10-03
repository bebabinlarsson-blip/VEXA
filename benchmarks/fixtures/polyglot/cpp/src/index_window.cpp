#include "../include/index_window.hpp"

bool IndexWindow::contains(std::size_t position) const {
    return position >= begin_ && position < end_;
}

