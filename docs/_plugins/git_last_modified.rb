# frozen_string_literal: true

require "open3"

# Dates every page and document by the last commit that touched its source
# file, as `last_modified_at`, which jekyll-sitemap writes as <lastmod> (and
# IndexNow uses to find changed pages). Explicit front matter wins, a post
# never dates before its own date, and a file Git cannot date (untracked, or
# outside the repository) gets no date rather than the build time. A page
# rendered from other files can name them, relative to the site source, in
# `last_modified_from` (say `[_data/projects.yml]`), and takes the latest of
# their dates. Needs the full history in CI (actions/checkout with
# fetch-depth: 0).
module GitLastModified
  module_function

  def apply(site)
    times = commit_times(site.source)
    return if times.empty?

    (site.pages + site.documents).each do |item|
      next if item.data.key?("last_modified_at")

      sources = [item.path, *Array(item.data["last_modified_from"])]
      time = sources.filter_map { |path| times[real_path(File.expand_path(path, site.source))] }.max
      next unless time

      date = item.data["date"]
      time = date if date.respond_to?(:to_time) && date.to_time > time
      item.data["last_modified_at"] = time
    end
  end

  def commit_times(source)
    root, status = Open3.capture2("git", "-C", source, "rev-parse", "--show-toplevel", err: File::NULL)
    return {} unless status.success?

    root = real_path(root.strip)
    log, status = Open3.capture2("git", "-C", root, "log", "--no-renames", "--format=%x1e%ct", "--name-only",
                                 err: File::NULL)
    return {} unless status.success?

    log.split("\x1e").each_with_object({}) do |entry, times|
      timestamp, *files = entry.split("\n").reject(&:empty?)
      next unless timestamp

      files.each { |file| times[File.join(root, file)] ||= Time.at(Integer(timestamp)) }
    end
  end

  def real_path(path)
    File.realpath(path)
  rescue SystemCallError
    path
  end
end

Jekyll::Hooks.register :site, :post_read do |site|
  GitLastModified.apply(site)
end
